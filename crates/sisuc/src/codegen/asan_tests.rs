//! The `ASan` test of `optimize` and `link`. Sisu has no way to write these bugs, so each row is
//! hand-written IR. It goes through the real `optimize` and `link`, and the executable runs.

use std::fs;
use std::process::Command;

use inkwell::context::Context;
use inkwell::memory_buffer::MemoryBuffer;
use inkwell::targets::TargetMachine;

use super::{OptLevel, object_code, optimize, target_machine};
use crate::link::{self, Linking, test_runtime};
use crate::{Allocator, Sanitizer, fresh_names, run};

/// The attributes that Sisu gives the allocator functions (`codegen/abi.rs`). The functions have
/// no `sanitize_address`, so that the test proves that `optimize` adds it to each function with a
/// body.
const PRELUDE: &str = "\
declare noalias ptr @sisu_alloc(i64, i64) nounwind allockind(\"alloc,uninitialized\") \"alloc-family\"=\"sisu\"
declare void @sisu_free(ptr allocptr captures(none), i64, i64) nounwind allockind(\"free\") \"alloc-family\"=\"sisu\"
declare void @sisu_print_str(ptr, i64)
";

/// A `@main` that runs `body`. Each bad access in a body is `volatile`, so that the optimizer
/// cannot remove it.
fn main_of(body: &str) -> String {
    format!("define i32 @main() {{\n{body}\n  ret i32 0\n}}\n")
}

/// One row: the bug, the levels to build at, the IR after the prelude, and the report that the
/// run must print.
struct Row {
    bug: &'static str,
    levels: &'static [OptLevel],
    ir: String,
    report: &'static str,
}

fn rows() -> Vec<Row> {
    const ALLOC: &str = "  %p = call ptr @sisu_alloc(i64 8, i64 8)\n";
    const FREE: &str = "  call void @sisu_free(ptr %p, i64 8, i64 8)\n";
    let (o0, o02) = (&[OptLevel::O0][..], &[OptLevel::O0, OptLevel::O2][..]);
    vec![
        Row {
            bug: "a store past a heap block",
            levels: o02,
            ir: main_of(&format!(
                "{ALLOC}  %g = getelementptr i8, ptr %p, i64 8\n  store volatile i64 1, ptr %g, align 8\n{FREE}"
            )),
            report: "heap-buffer-overflow",
        },
        Row {
            bug: "a load after sisu_free",
            levels: o02,
            ir: main_of(&format!(
                "{ALLOC}{FREE}  %v = load volatile i64, ptr %p, align 8"
            )),
            report: "heap-use-after-free",
        },
        Row {
            bug: "two sisu_free calls of a block",
            levels: o0,
            ir: main_of(&format!("{ALLOC}{FREE}{FREE}")),
            report: "attempting double-free",
        },
        Row {
            bug: "a store past an array in the entry block",
            levels: o0,
            ir: main_of(
                "  %a = alloca [4 x i64], align 8\n  %g = getelementptr i8, ptr %a, i64 32\n  store volatile i64 1, ptr %g, align 8",
            ),
            report: "stack-buffer-overflow",
        },
        Row {
            bug: "a load past a global",
            levels: o02,
            ir: format!(
                "@g = internal constant [8 x i8] zeroinitializer\n{}",
                main_of(
                    "  %x = getelementptr i8, ptr @g, i64 12\n  %v = load volatile i8, ptr %x, align 1"
                )
            ),
            report: "global-buffer-overflow",
        },
        Row {
            bug: "a load of a returned local's address",
            levels: o02,
            ir: format!(
                "define internal ptr @esc() noinline {{\n  %a = alloca [4 x i64], align 8\n  store volatile i64 7, ptr %a, align 8\n  ret ptr %a\n}}\n{}",
                main_of("  %e = call ptr @esc()\n  %v = load volatile i64, ptr %e, align 8")
            ),
            report: "stack-use-after-return",
        },
        Row {
            bug: "a too-long sisu_print_str of a heap block",
            levels: o0,
            ir: main_of(
                "  %p = call ptr @sisu_alloc(i64 8, i64 1)\n  call void @sisu_print_str(ptr %p, i64 16)\n  call void @sisu_free(ptr %p, i64 8, i64 1)",
            ),
            report: "heap-buffer-overflow",
        },
    ]
}

/// Builds `ir` for `ASan` at `level` and runs it. Returns why the run is not the expected one, or
/// `None` when it exited with a code other than 0 and stderr holds `report`.
fn failure_of(ir: &str, level: OptLevel, report: &str) -> Option<String> {
    let context = Context::create();
    let buffer =
        MemoryBuffer::create_from_memory_range_copy(format!("{PRELUDE}{ir}").as_bytes(), "asan");
    let module = match context.create_module_from_ir(buffer) {
        Ok(module) => module,
        Err(e) => return Some(format!("the IR does not parse: {e}")),
    };
    let machine = target_machine(level).expect("the host should have a target machine");
    // Without the triple of the host, the `asan` pass uses the wrong shadow offset.
    module.set_triple(&TargetMachine::get_default_triple());
    module.set_data_layout(&machine.get_target_data().get_data_layout());
    optimize(&module, &machine, level, Some(Sanitizer::Address))
        .expect("the module should optimize");
    let object = object_code(&module, &machine).expect("the module should compile");
    let dir = run::private_dir(&std::env::temp_dir(), fresh_names("-test"))
        .expect("the test should make its directory");
    let (object_path, exe) = (dir.join("asan.o"), dir.join("asan"));
    fs::write(&object_path, object.as_slice()).expect("the test should write the object");
    let linking = Linking {
        runtime: &test_runtime(Allocator::System),
        output: &exe,
        linker: None,
        sanitize: Some(Sanitizer::Address),
    };
    link::link(&[object_path], &linking).expect("the object should link");
    // The option is set so that the environment of the developer cannot turn off a report that
    // this test expects.
    let out = Command::new(&exe)
        .env("ASAN_OPTIONS", "fast_unwind_on_malloc=0")
        .output()
        .expect("the program should start");
    drop(fs::remove_dir_all(&dir));
    let stderr = String::from_utf8_lossy(&out.stderr);
    if out.status.code() == Some(0) {
        Some(format!("exited 0; stderr: {stderr}"))
    } else if !stderr.contains(report) {
        Some(format!("no `{report}` in stderr: {stderr}"))
    } else {
        None
    }
}

/// GIVEN an LLVM module with a bug
/// WHEN `optimize` at a level and `link` build it for `ASan`, and it runs
/// THEN it exits with a code other than 0
/// AND stderr holds the report of the bug
///
/// The rows are heap, stack and global errors, a use after return, a double free, and an over-read
/// by the runtime. A failure of one row does not stop the others: the test names each.
#[test]
#[cfg_attr(not(target_os = "linux"), ignore = "ASan works only on Linux")]
fn sanitized_ir_should_report_each_kind_of_memory_error() {
    let mut failures = Vec::new();
    for row in rows() {
        for &level in row.levels {
            if let Some(why) = failure_of(&row.ir, level, row.report) {
                let level = if level == OptLevel::O0 { "-O0" } else { "-O2" };
                failures.push(format!("{} at {level}: {why}", row.bug));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n---\n"));
}
