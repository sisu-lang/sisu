//! Methods on a type parameter: a call on a value of type `T` names a requirement of the
//! interfaces that `T`'s bounds give, and is an `InterfaceCall` that `mono` resolves.

use super::generic::{GenericSig, Site, mismatched, named_params};
use super::interface::Required;
use super::{Checked, Checker, Poisoned};
use crate::ast;
use crate::diagnostic::{Definition, DiagnosticKind};
use crate::source::Span;
use crate::tir::{self, Type, TypeArgs};

impl Checker<'_> {
    /// `receiver.method(args)` on a value of type `ty`, a type parameter or an associated type,
    /// which `site` writes: the call of the requirement that its bounds give. The call's own type
    /// arguments are inferred against the requirement, as for a generic method, and the bounds of
    /// the requirement's own parameters are checked.
    pub(super) fn bound_method(
        &mut self,
        span: Span,
        receiver: Checked,
        ty: Type,
        site: Site<'_>,
        args: &[ast::Arg],
    ) -> Checked {
        let found = self.required(ty, site.name.name);
        let Some(picked) = self.pick_required(ty, site, span, &found) else {
            self.labels(args);
            self.stray(args);
            return Err(Poisoned::error());
        };
        let Required {
            interface,
            args: iargs,
            method,
        } = picked;
        let labeled = self.labels(args);
        // The call keeps copies of the requirement while it checks its arguments, as `invoke`
        // does of a function.
        let info = &self.interfaces[interface.0];
        let requirement = &info.requirements[method];
        let (params, ret) = (requirement.params.clone(), requirement.ret);
        let name = format!("{}.{}", info.name.name, requirement.name.name);
        let mut declared = info.type_params.clone();
        declared.extend_from_slice(&requirement.own);
        let bounds = requirement.bounds.clone();
        let mut owner = vec![ty];
        owner.extend_from_slice(iargs.get());
        if params.len() != args.len() {
            self.stray(args);
            if !self.misread_in(args) {
                self.report(DiagnosticKind::ArgumentCount {
                    name,
                    want: params.len(),
                    found: args.len(),
                    at: span,
                });
            }
            return Err(Poisoned::error());
        }
        let sig = GenericSig {
            name: site.name,
            variant: None,
            declared: &declared,
            owner: &owner,
            params: &params,
            ret,
            bounds: &bounds,
        };
        let Some((checked, list, ret)) =
            self.generic_call(span, &sig, (site.written, site.angles), args, site.expected)
        else {
            return Err(Poisoned::error());
        };
        let (iargs, types) = list.get().split_at(owner.len());
        let (iargs, types) = (TypeArgs::intern(iargs), TypeArgs::intern(types));
        let failed = labeled || checked.iter().any(mismatched);
        match (std::iter::once(receiver).chain(checked).collect(), ret) {
            (Ok(args), Some(ret)) if !failed => {
                Ok(
                    self.spilled(span, ret, args, |args| tir::ExprKind::InterfaceCall {
                        interface,
                        iargs,
                        method: u32::try_from(method)
                            .expect("an interface should have fewer than 2^32 requirements"),
                        types,
                        args,
                    }),
                )
            }
            (_, ret) => Err(Poisoned::of(ret.filter(|_| !failed))),
        }
    }

    /// The requirement among `found`, which the bounds of `ty` give for the method `site` names,
    /// that the call at `span` takes. With none, reports that `ty` has no such method. With
    /// several, the call's context picks (`pick_among`).
    fn pick_required(
        &mut self,
        ty: Type,
        site: Site<'_>,
        span: Span,
        found: &[Required],
    ) -> Option<Required> {
        let wide = self.is_wide(ty);
        match found {
            // A parameter whose closure is too wide is reported already, and unknown: a method
            // that its walk did not reach is no error, and nor is one that it did, whatever the
            // call is.
            _ if wide => None,
            [] => {
                self.no_bound_method(ty, site.name);
                None
            }
            [one] => Some(*one),
            _ => {
                if self.context_failed(site, span) {
                    return None;
                }
                // A result that is `Error` has its own error.
                let results: Option<Vec<Type>> = (found.iter())
                    .map(|one| self.interfaces[one.interface.0].requirements[one.method].ret)
                    .collect();
                let results = results?;
                let written = self.written_list(site)?;
                let rets: Vec<Option<Type>> = (found.iter().zip(results))
                    .map(|(one, ret)| self.requirement_result(ty, one, ret, &written))
                    .collect();
                let interfaces = (found.iter())
                    .map(|one| self.bound_text(one.interface, one.args))
                    .collect();
                let picked = self.pick_among(ty, site, span, &rets, interfaces)?;
                Some(found[picked])
            }
        }
    }

    /// The type that the requirement `one` returns, `ret` in its own terms, for a call on `ty`
    /// that writes `written` for the requirement's own type parameters. `None` when `ret` names
    /// one that the call leaves to inference: the context cannot tell that candidate apart.
    pub(super) fn requirement_result(
        &self,
        ty: Type,
        one: &Required,
        ret: Type,
        written: &[Type],
    ) -> Option<Type> {
        let own = self.interfaces[one.interface.0].requirements[one.method]
            .own
            .len();
        let mut with = vec![ty];
        with.extend_from_slice(one.args.get());
        let base = with.len();
        let names = named_params(std::iter::once(Some(ret)), base + own);
        if !names[base..].contains(&true) {
            return self.subst(ret, &with);
        }
        if written.len() != own {
            return None;
        }
        with.extend_from_slice(written);
        self.subst(ret, &with)
    }

    /// Reports that the type parameter `ty` has no method `name`: no interface of its bounds
    /// declares one. The help adds the bound, when exactly one interface of the program declares
    /// a method of that name and it takes no type parameters.
    fn no_bound_method(&mut self, ty: Type, name: &ast::Ident) {
        let add = match self.declaring.get(&name.name).map(Vec::as_slice) {
            Some(&[only]) => self.add_bound(ty, only),
            _ => None,
        };
        self.report(DiagnosticKind::NoMember {
            owner: self.type_name(ty),
            kind: Definition::Method,
            name: name.name.to_string(),
            at: name.span,
            suggestion: None,
            add,
        });
    }
}
