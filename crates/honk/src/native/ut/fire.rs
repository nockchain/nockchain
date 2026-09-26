use super::*;

impl<'a> Ut<'a> {
    // `fine` resolves a `Port` to a native type and a formula ID. In `fire`, arm
    // cores are native types and each foot is a noun carrying the poly and the
    // hoon arm-spec. `sut` is the subject `++fine` runs against (hoon-138's
    // `sut`); `fire` keys its wet-arm recursion guard on it.
    pub(super) fn fine(&mut self, sut: &NRc<NTy>, port: &Port) -> Result<(NRc<NTy>, FormulaId)> {
        match port {
            Port::Synthetic { typ, formula } => Ok((typ.clone(), *formula)),
            Port::Palo(palo) => match &palo.opal {
                Opal::Leg(typ) => {
                    let axis = tend_big(&palo.vein)?;
                    Ok((typ.clone(), self.formula_slot(axis)))
                }
                Opal::Arm { axis, arms } => {
                    let axe = tend_big(&palo.vein)?;
                    let ty = self.fire(sut, arms)?;
                    let slot = self.formula_slot(axe);
                    let formula = self.formula_arena.kick(axis.clone(), slot);
                    Ok((ty, formula))
                }
            },
        }
    }

    /// hoon-138 `++fire`. `sut` is the call-site subject, the `sut` of the
    /// `++fire` call: the wet-arm recursion guard `rib` is keyed on
    /// `[sut dox arm]`, while the arm body is mulled against the redone core.
    pub(super) fn fire(&mut self, sut: &NRc<NTy>, arms: &[(NRc<NTy>, Noun)]) -> Result<NRc<NTy>> {
        self.fire_with_mode(sut, arms, false)
    }

    fn fire_is_wet_axis_one(&mut self, hoon: Noun) -> bool {
        self.hoon_ast_lookup(hoon)
            .map(|ast| matches!(ast.as_ref(), Hoon::Axis(axis) if axis.is_one()))
            .unwrap_or(false)
    }

    fn fire_arm_dry(
        &mut self,
        arm_core: NRc<NTy>,
        hoon: Noun,
        dry_vet_checks_active: bool,
    ) -> Result<NRc<NTy>> {
        let NTy::Core {
            payload,
            garb,
            context,
            rest,
        } = &*arm_core
        else {
            return Err(CompilerError::Noun("fire-core".to_string()));
        };
        let payload = payload.clone();
        let context = context.clone();
        if dry_vet_checks_active && !self.nest(context.clone(), payload)? {
            return Err(CompilerError::Noun("fire-dry".to_string()));
        }
        let dox = self.core_dox_native(garb, &context, rest)?;
        // `[%hold dox hoon]`, interned to the same type that `native_of` gives
        // for the noun `ty_hold_cached(dox, hoon)`.
        Ok(self.cons_hold(dox, hoon))
    }

    fn fire_arm_wet(&mut self, sut: &NRc<NTy>, arm_core: NRc<NTy>, hoon: Noun) -> Result<NRc<NTy>> {
        let NTy::Core {
            payload,
            garb,
            context,
            rest,
        } = &*arm_core
        else {
            return Err(CompilerError::Noun("fire-core".to_string()));
        };
        let payload = payload.clone();
        let garb = garb.clone();
        let context = context.clone();
        let rest = rest.clone();
        // `redo_wet_payload` takes and returns nouns: lower payload and context,
        // redo, then lift the redone payload back to native.
        let payload_noun = live_to_noun(&mut self.cx, &payload, self.slab);
        let context_noun = live_to_noun(&mut self.cx, &context, self.slab);
        let redone_payload_noun = self.redo_wet_payload(payload_noun, context_noun)?;
        let redone_payload = self.native_of_cached(redone_payload_noun)?;
        // Same garb, context, and rest; new payload.
        let redone_core = cons_core(
            &mut self.cx,
            redone_payload,
            garb.clone(),
            context.clone(),
            rest.clone(),
        );
        let dox = self.core_dox_native(&garb, &context, &rest)?;
        self.mull_check_wet(sut, redone_core.clone(), dox, hoon)?;
        // `[%hold redone_core hoon]`, interned the same way as in `fire_arm_dry`.
        Ok(self.cons_hold(redone_core, hoon))
    }

    fn fire_with_mode(
        &mut self,
        sut: &NRc<NTy>,
        arms: &[(NRc<NTy>, Noun)],
        skip_vet_dry_checks: bool,
    ) -> Result<NRc<NTy>> {
        if arms.is_empty() {
            return Ok(cons_void(&mut self.cx));
        }
        if arms.len() == 1 {
            let space = self.slab.noun_space();
            let (core, foot) = &arms[0];
            if let Ok((poly, hoon)) = foot_parts(*foot, &space) {
                if poly == Poly::Wet && self.fire_is_wet_axis_one(hoon) {
                    return Ok(core.clone());
                }
            }
        }
        let mut options = Vec::with_capacity(arms.len());
        for (core, foot) in arms {
            let space = self.slab.noun_space();
            let (poly, hoon) = foot_parts(*foot, &space)?;
            let dry_vet_checks_active = poly == Poly::Dry && self.vet && !skip_vet_dry_checks;
            if !matches!(&**core, NTy::Core { .. }) {
                return Err(CompilerError::Noun("fire-core".to_string()));
            }
            let arm_ty = match poly {
                Poly::Dry => self.fire_arm_dry(core.clone(), hoon, dry_vet_checks_active)?,
                Poly::Wet => self.fire_arm_wet(sut, core.clone(), hoon)?,
            };
            options.push(arm_ty);
        }
        if options.len() == 1 {
            Ok(options.pop().expect("single fire option"))
        } else {
            self.cons_fork(options)
        }
    }

    /// Builds the interned `[%hold inner hoon]`. The gene is the raw arm hoon, as
    /// `++fire` (hoon-138.hoon:9529) stores it; `++open` lowering happens only
    /// when the hold is forced (repo/rest/play).
    fn cons_hold(&mut self, inner: NRc<NTy>, hoon: Noun) -> NRc<NTy> {
        let gene = live_leaf_from_noun(&mut self.cx, hoon, &self.slab.noun_space());
        live_intern(
            &mut self.cx,
            NTy::Hold {
                subject: inner,
                gene,
            },
        )
    }

    // Noun counterpart of `core_dox_native`, exercised by the
    // `core_dox_uses_context_payload` test.
    #[cfg(test)]
    pub(super) fn core_dox(&mut self, core: Noun) -> Result<Noun> {
        let space = self.slab.noun_space();
        let (_payload, coil) = type_core_parts(core, &space)?;
        let (garb, context, rest) = coil_parts(coil, &space)?;
        let new_garb = self.garb_with_vair(garb, Vair::Gold)?;
        let new_coil = coil_from_parts(self.slab, new_garb, context, rest);
        Ok(ty_core(self.slab, context, new_coil))
    }

    pub(super) fn garb_with_vair(&mut self, garb: Noun, vair: Vair) -> Result<Noun> {
        let space = self.slab.noun_space();
        let (nym, poly, _old_vair) = garb_parts(garb, &space)?;
        let vair_noun = term_to_noun(
            self.slab,
            match vair {
                Vair::Gold => "gold",
                Vair::Iron => "iron",
                Vair::Lead => "lead",
                Vair::Zinc => "zinc",
            },
        );
        let tail = T(self.slab, &[poly, vair_noun]);
        Ok(T(self.slab, &[nym, tail]))
    }
}
