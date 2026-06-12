use super::*;

impl<'a> Ut<'a> {
    pub(super) fn fine(&mut self, port: &Port) -> Result<(Noun, Noun)> {
        match port {
            Port::Synthetic { typ, formula } => Ok((*typ, *formula)),
            Port::Palo(palo) => match &palo.opal {
                Opal::Leg(typ) => {
                    let axis = tend_big(&palo.vein)?;
                    Ok((*typ, slot_formula_axis_big(self.slab, axis)))
                }
                Opal::Arm { axis, arms } => {
                    let axe = tend_big(&palo.vein)?;
                    let ty = self.fire(arms)?;
                    let slot = slot_formula_axis_big(self.slab, axe);
                    let arm_axis_noun = noun_u64(self.slab, *axis);
                    let formula = T(self.slab, &[D(9), arm_axis_noun, slot]);
                    Ok((ty, formula))
                }
            },
        }
    }

    pub(super) fn fire(&mut self, arms: &[(Noun, Noun)]) -> Result<Noun> {
        self.fire_with_mode(arms, false)
    }

    fn fire_is_wet_axis_one(&mut self, hoon: Noun) -> bool {
        self.hoon_ast_lookup(hoon)
            .map(|ast| matches!(ast.as_ref(), Hoon::Axis(1)))
            .unwrap_or(false)
    }

    fn fire_arm_dry(
        &mut self,
        arm_core: Noun,
        hoon: Noun,
        dry_vet_checks_active: bool,
    ) -> Result<Noun> {
        let space = self.slab.noun_space();
        let (payload, coil) = type_core_parts(arm_core, &space)?;
        let (_garb, context, _rest) = coil_parts(coil, &space)?;
        if dry_vet_checks_active && !self.nest(context, payload)? {
            return Err(CompilerError::Noun("fire-dry".to_string()));
        }
        let dox = self.core_dox(arm_core)?;
        self.ty_hold_cached(dox, hoon)
    }

    fn fire_arm_wet(&mut self, arm_core: Noun, hoon: Noun) -> Result<Noun> {
        let space = self.slab.noun_space();
        let (payload, coil) = type_core_parts(arm_core, &space)?;
        let (garb, context, rest) = coil_parts(coil, &space)?;
        let redone_payload = self.redo_wet_payload(payload, context)?;
        let redone_coil = coil_from_parts(self.slab, garb, context, rest);
        let redone_core = ty_core(self.slab, redone_payload, redone_coil);
        let dox = self.core_dox(arm_core)?;
        self.mull_check_wet(redone_core, dox, hoon)?;
        self.ty_hold_cached(redone_core, hoon)
    }

    fn fire_with_mode(&mut self, arms: &[(Noun, Noun)], skip_vet_dry_checks: bool) -> Result<Noun> {
        if arms.is_empty() {
            return Ok(ty_void(self.slab));
        }
        if arms.len() == 1 {
            let space = self.slab.noun_space();
            let (core, foot) = arms[0];
            if let Ok((poly, hoon)) = foot_parts(foot, &space) {
                if poly == Poly::Wet && self.fire_is_wet_axis_one(hoon) {
                    return Ok(core);
                }
            }
        }
        let mut options = Vec::with_capacity(arms.len());
        for (core, foot) in arms {
            let space = self.slab.noun_space();
            let (poly, hoon) = foot_parts(*foot, &space)?;
            let dry_vet_checks_active = poly == Poly::Dry && self.vet && !skip_vet_dry_checks;
            let arm_core = *core;
            let tag = type_tag(arm_core, &space)?;
            if tag.as_str() != "core" {
                return Err(CompilerError::Noun("fire-core".to_string()));
            }
            let arm_ty = match poly {
                Poly::Dry => self.fire_arm_dry(arm_core, hoon, dry_vet_checks_active)?,
                Poly::Wet => self.fire_arm_wet(arm_core, hoon)?,
            };
            options.push(arm_ty);
        }
        let out = if options.len() == 1 {
            options[0]
        } else {
            self.fork_from_options(options)?
        };
        Ok(out)
    }

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
