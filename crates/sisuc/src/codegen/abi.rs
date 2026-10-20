//! The runtime's C ABI as codegen sees it: one row for each function that `crates/runtime`
//! exports, with the types and attributes codegen declares it with. The tests read the
//! runtime's source and check every row against the Rust signature it describes.

use inkwell::AddressSpace;
use inkwell::attributes::{Attribute, AttributeLoc};
use inkwell::context::ContextRef;
use inkwell::module::Module;
use inkwell::types::{BasicMetadataTypeEnum, BasicType, BasicTypeEnum};
use inkwell::values::FunctionValue;

/// How a value crosses into the runtime. `Bool` and `U8` are passed `zeroext`, as the C
/// ABI wants for integers narrower than 32 bits: the callee may read all 32 bits of the
/// argument, so the caller must extend it with zeros.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Abi {
    I64,
    I32,
    U8,
    Bool,
    F64,
    Ptr,
}

impl Abi {
    fn llvm_type(self, context: ContextRef<'_>) -> BasicTypeEnum<'_> {
        match self {
            Abi::I64 => context.i64_type().into(),
            Abi::I32 => context.i32_type().into(),
            Abi::U8 => context.i8_type().into(),
            Abi::Bool => context.bool_type().into(),
            Abi::F64 => context.f64_type().into(),
            Abi::Ptr => context.ptr_type(AddressSpace::default()).into(),
        }
    }
}

/// What LLVM may assume about a call, beyond its type. `memory` is an LLVM `memory` attribute
/// value, in LLVM 23's `MemoryEffects` encoding (`llvm/Support/ModRef.h`): two bits per
/// location, `Ref` 1 and `Mod` 2, lowest first: `ArgMem`, `InaccessibleMem`, `ErrnoMem`, then
/// everything else (`Other` and the target locations, globals included).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Attrs {
    pub(super) nounwind: bool,
    pub(super) noreturn: bool,
    pub(super) memory: Option<u64>,
    pub(super) allocator: Allocator,
}

/// A function's part in LLVM's allocator model, which lets `-O2` delete an allocation that
/// nothing reads and pair it with its free.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Allocator {
    None,
    /// Returns fresh, uninitialized memory that nothing else points to.
    Alloc,
    /// Frees its first parameter and keeps no copy of it.
    Free,
}

/// One function the runtime exports: its name, its parameters, its return value (`None` for
/// none, or for `!`), and its attributes.
pub(super) struct RuntimeFn {
    pub(super) name: &'static str,
    pub(super) params: &'static [Abi],
    pub(super) ret: Option<Abi>,
    pub(super) attrs: Attrs,
}

const NONE: Attrs = Attrs {
    nounwind: false,
    noreturn: false,
    memory: None,
    allocator: Allocator::None,
};
/// The runtime is built with `panic = "abort"`, so it never unwinds.
const NOUNWIND: Attrs = Attrs {
    nounwind: true,
    ..NONE
};
const NORETURN: Attrs = Attrs {
    noreturn: true,
    ..NONE
};
/// `sisu_alloc`: `NOUNWIND`, and the allocator that `sisu_free` frees.
const ALLOC: Attrs = Attrs {
    allocator: Allocator::Alloc,
    ..NOUNWIND
};
/// `sisu_free`: `NOUNWIND`, and the free of what `sisu_alloc` returns.
const FREE: Attrs = Attrs {
    allocator: Allocator::Free,
    ..NOUNWIND
};

/// `NOUNWIND`, with the `memory` attribute value `memory`.
const fn nounwind_memory(memory: u64) -> Attrs {
    Attrs {
        memory: Some(memory),
        ..NOUNWIND
    }
}

/// Touches no memory.
const NO_MEMORY: u64 = 0;
/// Reads what its pointer arguments point to (`ArgMem` `Ref`), and no other memory.
const READS_ARGS: u64 = 0b01;
/// Reads and writes what its pointer arguments point to (`ArgMem` `Ref` and `Mod`), and no
/// other memory.
const ARGS: u64 = 0b11;
/// A print function's effects, without its pointer arguments: `ErrnoMem` 3, `InaccessibleMem`
/// 3, `ArgMem` 0, and none for the rest. Of the module's memory, a print reads only what a
/// pointer parameter points to (`PRINT | READS_ARGS`); otherwise it uses only the runtime's own
/// buffer and state (inaccessible memory) and can set `errno`. It can also exit the process, so
/// there is no `willreturn`. Treating the runtime's allocator as inaccessible memory matches
/// LLVM's own annotation of `malloc`. These attributes assume no LTO, no `atexit` handlers and
/// no module globals that the runtime reaches.
const PRINT: u64 = 0b11_11_00;

/// Every function the runtime exports, by runtime file, then by name.
pub(super) const RUNTIME: &[RuntimeFn] = &[
    // args.rs
    RuntimeFn {
        name: "sisu_arg",
        params: &[Abi::Ptr, Abi::I64, Abi::Ptr],
        ret: Some(Abi::Ptr),
        attrs: NOUNWIND,
    },
    RuntimeFn {
        name: "sisu_args_len",
        params: &[Abi::I32],
        ret: Some(Abi::I64),
        attrs: nounwind_memory(NO_MEMORY),
    },
    // float.rs
    RuntimeFn {
        name: "sisu_write_f64",
        params: &[Abi::F64, Abi::Ptr],
        ret: Some(Abi::I64),
        attrs: NONE,
    },
    RuntimeFn {
        name: "sisu_write_f64_fixed",
        params: &[Abi::F64, Abi::U8, Abi::Ptr],
        ret: Some(Abi::I64),
        attrs: NONE,
    },
    // lib.rs
    RuntimeFn {
        name: "sisu_alloc",
        params: &[Abi::I64, Abi::I64],
        ret: Some(Abi::Ptr),
        attrs: ALLOC,
    },
    RuntimeFn {
        name: "sisu_eprint_bool",
        params: &[Abi::Bool],
        ret: None,
        attrs: nounwind_memory(PRINT),
    },
    RuntimeFn {
        name: "sisu_eprint_f64",
        params: &[Abi::F64],
        ret: None,
        attrs: nounwind_memory(PRINT),
    },
    RuntimeFn {
        name: "sisu_eprint_int",
        params: &[Abi::I64],
        ret: None,
        attrs: nounwind_memory(PRINT),
    },
    RuntimeFn {
        name: "sisu_flush_stdout",
        params: &[],
        ret: None,
        attrs: NONE,
    },
    RuntimeFn {
        name: "sisu_free",
        params: &[Abi::Ptr, Abi::I64, Abi::I64],
        ret: None,
        attrs: FREE,
    },
    RuntimeFn {
        name: "sisu_panic",
        params: &[Abi::Ptr, Abi::I64],
        ret: None,
        attrs: NORETURN,
    },
    RuntimeFn {
        name: "sisu_panic_bounds",
        params: &[Abi::Ptr, Abi::I64, Abi::I64, Abi::I64],
        ret: None,
        attrs: NORETURN,
    },
    RuntimeFn {
        name: "sisu_panic_message",
        params: &[Abi::Ptr, Abi::I64, Abi::Ptr, Abi::I64],
        ret: None,
        attrs: NORETURN,
    },
    RuntimeFn {
        name: "sisu_print_bool",
        params: &[Abi::Bool],
        ret: None,
        attrs: nounwind_memory(PRINT),
    },
    RuntimeFn {
        name: "sisu_print_f64",
        params: &[Abi::F64],
        ret: None,
        attrs: nounwind_memory(PRINT),
    },
    RuntimeFn {
        name: "sisu_print_int",
        params: &[Abi::I64],
        ret: None,
        attrs: nounwind_memory(PRINT),
    },
    RuntimeFn {
        name: "sisu_rc_check_exit",
        params: &[],
        ret: None,
        attrs: NONE,
    },
    RuntimeFn {
        name: "sisu_rc_fault",
        params: &[Abi::I32],
        ret: None,
        attrs: NORETURN,
    },
    RuntimeFn {
        name: "sisu_realloc",
        params: &[Abi::Ptr, Abi::I64, Abi::I64, Abi::I64],
        ret: Some(Abi::Ptr),
        attrs: NONE,
    },
    // stdin.rs
    RuntimeFn {
        name: "sisu_read_line",
        params: &[Abi::Ptr],
        ret: Some(Abi::Ptr),
        attrs: NOUNWIND,
    },
    // text.rs
    RuntimeFn {
        name: "sisu_eprint_char",
        params: &[Abi::I32],
        ret: None,
        attrs: nounwind_memory(PRINT),
    },
    RuntimeFn {
        name: "sisu_eprint_str",
        params: &[Abi::Ptr, Abi::I64],
        ret: None,
        attrs: nounwind_memory(PRINT | READS_ARGS),
    },
    RuntimeFn {
        name: "sisu_pad",
        params: &[Abi::Ptr, Abi::I64, Abi::I64, Abi::U8],
        ret: Some(Abi::I64),
        attrs: NONE,
    },
    RuntimeFn {
        name: "sisu_print_char",
        params: &[Abi::I32],
        ret: None,
        attrs: nounwind_memory(PRINT),
    },
    RuntimeFn {
        name: "sisu_print_str",
        params: &[Abi::Ptr, Abi::I64],
        ret: None,
        attrs: nounwind_memory(PRINT | READS_ARGS),
    },
    // A pure function of the bytes it reads.
    RuntimeFn {
        name: "sisu_str_compare",
        params: &[Abi::Ptr, Abi::I64, Abi::Ptr, Abi::I64],
        ret: Some(Abi::I64),
        attrs: nounwind_memory(READS_ARGS),
    },
    RuntimeFn {
        name: "sisu_utf8_count",
        params: &[Abi::Ptr, Abi::I64],
        ret: Some(Abi::I64),
        attrs: NONE,
    },
    RuntimeFn {
        name: "sisu_utf8_decode",
        params: &[Abi::Ptr, Abi::I64, Abi::Ptr],
        ret: None,
        attrs: NONE,
    },
    RuntimeFn {
        name: "sisu_utf8_encode",
        params: &[Abi::Ptr, Abi::I64, Abi::Ptr],
        ret: None,
        attrs: NONE,
    },
    RuntimeFn {
        name: "sisu_utf8_len",
        params: &[Abi::Ptr, Abi::I64],
        ret: Some(Abi::I64),
        attrs: NONE,
    },
    // Reads the bytes, and writes the word's start.
    RuntimeFn {
        name: "sisu_word_next",
        params: &[Abi::Ptr, Abi::I64, Abi::I64, Abi::Ptr],
        ret: Some(Abi::I64),
        attrs: nounwind_memory(ARGS),
    },
    RuntimeFn {
        name: "sisu_words_count",
        params: &[Abi::Ptr, Abi::I64],
        ret: Some(Abi::I64),
        attrs: nounwind_memory(READS_ARGS),
    },
    RuntimeFn {
        name: "sisu_write_bool",
        params: &[Abi::Bool, Abi::Ptr],
        ret: Some(Abi::I64),
        attrs: NONE,
    },
    RuntimeFn {
        name: "sisu_write_char",
        params: &[Abi::I32, Abi::Ptr],
        ret: Some(Abi::I64),
        attrs: NONE,
    },
    RuntimeFn {
        name: "sisu_write_int",
        params: &[Abi::I64, Abi::Ptr],
        ret: Some(Abi::I64),
        attrs: NONE,
    },
];

/// `sisu_rc_fault`'s `kind` for a retain of an object whose count is below 1, for a release of
/// one, and for a read of a local after it was moved. The runtime reads `kind` in
/// `sisu_rc_fault` (`crates/runtime/src/lib.rs`).
pub(super) const RC_FAULT_RETAIN: u64 = 0;
pub(super) const RC_FAULT_RELEASE: u64 = 1;
pub(super) const RC_FAULT_MOVED: u64 = 2;

/// `sisu_pad`'s `align`: the text goes on the left, on the right, or in the centre. The runtime
/// reads `align` in `sisu_pad` (`crates/runtime/src/text.rs`).
pub(super) const PAD_LEFT: u64 = 0;
pub(super) const PAD_RIGHT: u64 = 1;
pub(super) const PAD_CENTER: u64 = 2;

/// The row of `RUNTIME` named `name`.
pub(super) fn row(name: &str) -> &'static RuntimeFn {
    RUNTIME
        .iter()
        .find(|row| row.name == name)
        .unwrap_or_else(|| panic!("{name} should be a row of abi::RUNTIME"))
}

/// Declares `row`'s function in `module`, with its types and attributes.
pub(super) fn declare<'ctx>(module: &Module<'ctx>, row: &RuntimeFn) -> FunctionValue<'ctx> {
    let context = module.get_context();
    let params: Vec<BasicMetadataTypeEnum> = row
        .params
        .iter()
        .map(|abi| abi.llvm_type(context).into())
        .collect();
    let fn_type = match row.ret {
        Some(ret) => ret.llvm_type(context).fn_type(&params, false),
        None => context.void_type().fn_type(&params, false),
    };
    let function = module.add_function(row.name, fn_type, None);
    let attribute =
        |name, value| context.create_enum_attribute(Attribute::get_named_enum_kind_id(name), value);
    for (i, abi) in (0..).zip(row.params) {
        if matches!(abi, Abi::Bool | Abi::U8) {
            function.add_attribute(AttributeLoc::Param(i), attribute("zeroext", 0));
        }
    }
    if row.attrs.nounwind {
        function.add_attribute(AttributeLoc::Function, attribute("nounwind", 0));
    }
    if let Some(memory) = row.attrs.memory {
        function.add_attribute(AttributeLoc::Function, attribute("memory", memory));
    }
    if row.attrs.noreturn {
        function.add_attribute(AttributeLoc::Function, attribute("noreturn", 0));
    }
    let family = || context.create_string_attribute("alloc-family", "sisu");
    match row.attrs.allocator {
        Allocator::None => {}
        Allocator::Alloc => {
            function.add_attribute(AttributeLoc::Return, attribute("noalias", 0));
            // `AllocFnKind` `Alloc` 1 | `Uninitialized` 8.
            function.add_attribute(AttributeLoc::Function, attribute("allockind", 0b1001));
            function.add_attribute(AttributeLoc::Function, family());
        }
        Allocator::Free => {
            function.add_attribute(AttributeLoc::Param(0), attribute("allocptr", 0));
            // `CaptureComponents::None`.
            function.add_attribute(AttributeLoc::Param(0), attribute("captures", 0));
            // `AllocFnKind` `Free` 4.
            function.add_attribute(AttributeLoc::Function, attribute("allockind", 0b100));
            function.add_attribute(AttributeLoc::Function, family());
        }
    }
    function
}

#[cfg(test)]
mod tests {
    use super::*;
    use inkwell::context::Context;
    use inkwell::values::AnyValue;
    use std::collections::BTreeSet;

    /// Every `crates/runtime/src/*.rs`, parsed.
    fn runtime_files() -> Vec<syn::File> {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../runtime/src");
        let files: Vec<syn::File> = std::fs::read_dir(dir)
            .expect("the runtime's source directory should be readable")
            .map(|entry| {
                entry
                    .expect("a runtime source entry should be readable")
                    .path()
            })
            .filter(|path| path.extension().is_some_and(|ext| ext == "rs"))
            .map(|path| {
                let source = std::fs::read_to_string(&path).expect("a runtime file should read");
                syn::parse_file(&source).expect("a runtime file should parse")
            })
            .collect();
        assert!(!files.is_empty(), "no runtime source under {dir}");
        files
    }

    /// The signatures of the functions in `items` that carry `#[unsafe(no_mangle)]`, in inline
    /// modules too.
    fn runtime_exports(items: &[syn::Item]) -> Vec<&syn::Signature> {
        let no_mangle = |attr: &syn::Attribute| {
            matches!(&attr.meta, syn::Meta::List(list)
                if list.path.is_ident("unsafe") && list.tokens.to_string() == "no_mangle")
        };
        items
            .iter()
            .flat_map(|item| match item {
                syn::Item::Fn(function) if function.attrs.iter().any(no_mangle) => {
                    vec![&function.sig]
                }
                syn::Item::Mod(module) => module
                    .content
                    .as_ref()
                    .map_or_else(Vec::new, |(_, items)| runtime_exports(items)),
                _ => Vec::new(),
            })
            .collect()
    }

    /// The `Abi` of the Rust type `ty`, if it has one.
    fn abi_of(ty: &syn::Type) -> Option<Abi> {
        match ty {
            syn::Type::Ptr(_) => Some(Abi::Ptr),
            syn::Type::Path(path) => match path.path.segments.last()?.ident.to_string().as_str() {
                "i64" | "u64" | "usize" | "isize" => Some(Abi::I64),
                "i32" | "u32" => Some(Abi::I32),
                "u8" => Some(Abi::U8),
                "bool" => Some(Abi::Bool),
                "f64" => Some(Abi::F64),
                _ => None,
            },
            _ => None,
        }
    }

    #[test]
    fn every_runtime_export_has_a_row_and_every_row_an_export() {
        let files = runtime_files();
        let exports: BTreeSet<String> = files
            .iter()
            .flat_map(|file| runtime_exports(&file.items))
            .map(|sig| sig.ident.to_string())
            .collect();
        let rows: BTreeSet<String> = RUNTIME.iter().map(|row| row.name.to_owned()).collect();
        assert_eq!(
            rows.len(),
            RUNTIME.len(),
            "RUNTIME has a duplicated row: `row()` would return only the first",
        );
        assert!(
            exports == rows,
            "exports with no row: {:?}; rows with no export: {:?}",
            exports.difference(&rows).collect::<Vec<_>>(),
            rows.difference(&exports).collect::<Vec<_>>(),
        );
    }

    #[test]
    fn each_row_matches_its_rust_signature() {
        let files = runtime_files();
        for sig in files.iter().flat_map(|file| runtime_exports(&file.items)) {
            let name = sig.ident.to_string();
            let row = row(&name);
            let params: Vec<Abi> = (1..)
                .zip(&sig.inputs)
                .map(|(position, input)| {
                    let syn::FnArg::Typed(input) = input else {
                        panic!("{name}: parameter {position} is a `self` receiver");
                    };
                    abi_of(&input.ty).unwrap_or_else(|| {
                        panic!("{name}: parameter {position} has a type with no Abi")
                    })
                })
                .collect();
            let (ret, noreturn) = match &sig.output {
                syn::ReturnType::Default => (None, false),
                syn::ReturnType::Type(_, ty) if matches!(**ty, syn::Type::Never(_)) => (None, true),
                syn::ReturnType::Type(_, ty) => {
                    let abi =
                        abi_of(ty).unwrap_or_else(|| panic!("{name}: the return type has no Abi"));
                    (Some(abi), false)
                }
            };
            assert_eq!(row.params, params, "{name}: parameters");
            assert_eq!(row.ret, ret, "{name}: return value");
            assert_eq!(row.attrs.noreturn, noreturn, "{name}: noreturn");
        }
    }

    #[test]
    fn declares_what_the_row_says() {
        let context = Context::create();
        let module = context.create_module("abi");
        let print_bool = declare(&module, row("sisu_print_bool"))
            .print_to_string()
            .to_string();
        for wanted in ["zeroext", "nounwind", "memory("] {
            assert!(
                print_bool.contains(wanted),
                "missing {wanted:?} in\n{print_bool}"
            );
        }
        let panic = declare(&module, row("sisu_panic"))
            .print_to_string()
            .to_string();
        assert!(
            panic.contains("noreturn"),
            "missing \"noreturn\" in\n{panic}"
        );
    }
}
