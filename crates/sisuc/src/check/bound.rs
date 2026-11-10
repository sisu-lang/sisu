//! Methods on a type parameter: a call on a value of type `T` names a requirement of the
//! interfaces that `T`'s bounds give, and is an `InterfaceCall` that `mono` resolves.

use super::generic::{GenericSig, Site, mismatched, named_params, written_args};
use super::interface::Required;
use super::mut_args::{MutParts, MutSite, site_of};
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
        let mut found = self.required(ty, site.name.name);
        // Every type parameter has a `Display`, with no bound (the spec's "Every type parameter
        // prints"), so `to_string` is always a call of it. `mono` lowers it for the type argument.
        if found.is_empty() && site.name.name.as_str() == "to_string" {
            found.push(Required {
                interface: self.lang.display,
                args: TypeArgs::NONE,
                method: 0,
            });
        }
        // A rejected block may have given the type the method (see `Checker.rejected_methods`).
        if found.is_empty() && self.rejected_methods.contains(&site.name.name) {
            return self.call_of_rejected_method(site.name, args);
        }
        let Some(picked) = self.pick_required(ty, site, span, &found) else {
            self.stamp_candidates(&found, args);
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
        // Only a requirement with a `mut` parameter, or a call with a `mut` argument, pays for a
        // site.
        let (muts, names) = site_of(&requirement.muts, &requirement.param_names, args);
        let name = format!("{}.{}", info.name.name, requirement.name.name);
        let mut declared = info.type_params.clone();
        declared.extend_from_slice(&requirement.own);
        let bounds = requirement.bounds.clone();
        let mut owner = vec![ty];
        owner.extend_from_slice(iargs.get());
        self.stamp_mut_params(args, &muts);
        if params.len() != args.len() {
            let fixed = self.counted_result(ret, &declared, &owner, &bounds, site);
            return self.wrong_count(span, name, params.len(), args, fixed);
        }
        let mut_site = MutSite {
            callee: &name,
            muts: &muts,
            names: &names,
        };
        let sig = GenericSig {
            name: site.name,
            variant: None,
            declared: &declared,
            owner: &owner,
            params: &params,
            ret,
            bounds: &bounds,
            mut_site: (!muts.is_empty()).then_some(&mut_site),
        };
        let mut parts = MutParts::default();
        let written = (site.written, site.angles);
        let result = self.generic_call_mut(span, &sig, written, args, site.expected, &mut parts);
        let list = result.as_ref().ok().map(|(_, list, _)| *list);
        let met = (
            site.expected.ty(),
            result.as_ref().ok().and_then(|(_, _, ret)| *ret),
        );
        self.marks(args, &mut parts, &mut_site, (&params, list), met);
        let (checked, list, solved) = result?;
        let (iargs, types) = list.get().split_at(owner.len());
        let (iargs, types) = (TypeArgs::intern(iargs), TypeArgs::intern(types));
        let parts = self.mut_overlaps(parts);
        let failed = labeled || checked.iter().any(mismatched);
        let parted = labeled || checked.iter().any(Result::is_err);
        let method =
            u32::try_from(method).expect("an interface should have fewer than 2^32 requirements");
        let call = move |args| tir::ExprKind::InterfaceCall {
            interface,
            iargs,
            method,
            types,
            args,
        };
        match (std::iter::once(receiver).chain(checked).collect(), solved) {
            (Ok(args), Some(solved)) if !failed && parts.is_empty() => {
                Ok(self.spilled(span, solved, args, call))
            }
            (Ok(args), Some(solved)) if !failed => {
                let call = tir::Expr::new(call(args), solved, span);
                Ok(self.mut_call(call, parts))
            }
            _ => {
                let written = written_args(site, list.get(), owner.len());
                let sig = (ret, declared.len(), &owner[..]);
                Err(self.failed_call((solved, parted), sig, written))
            }
        }
    }

    /// Stamps the arguments at a `mut` parameter of any of the `found` candidates. The call
    /// picks none of them. Each argument gets the help of a place.
    fn stamp_candidates(&mut self, found: &[Required], args: &[ast::Arg]) {
        for one in found {
            let muts = self.interfaces[one.interface.0].requirements[one.method]
                .muts
                .clone();
            self.stamp_mut_params(args, &muts);
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
        let unknown = self.is_unknown(ty);
        match found {
            // A parameter whose closure is too wide, or whose bound failed, is reported already,
            // and unknown: a method that its walk did not reach is no error, and nor is one that
            // it did, whatever the call is.
            _ if unknown => None,
            [] => {
                self.no_bound_method(ty, site.name);
                None
            }
            [one] => Some(*one),
            _ => {
                if Self::context_failed(site) {
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
        #[cfg(test)]
        super::reached("requirement_result");
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
        let d = DiagnosticKind::NoMember {
            owner: self.type_name(ty),
            kind: Definition::Method,
            name: name.name.to_string(),
            at: name.span,
            suggestion: None,
            add,
        };
        self.report_lookup_failure(d.into(), name);
    }
}
