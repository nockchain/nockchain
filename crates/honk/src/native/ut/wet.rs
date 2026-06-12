use super::*;

#[derive(Clone)]
pub(super) struct RedoState {
    hos: Vec<Noun>,
    wec: Vec<Vec<Noun>>,
    gil: Vec<(Noun, Noun)>,
}

impl Default for RedoState {
    fn default() -> Self {
        Self {
            hos: Vec::new(),
            wec: vec![Vec::new()],
            gil: Vec::new(),
        }
    }
}

impl RedoState {
    fn for_cell_descent(&self) -> Self {
        Self {
            hos: Vec::new(),
            wec: vec![Vec::new()],
            gil: self.gil.clone(),
        }
    }
}

impl<'a> Ut<'a> {
    fn redo_fork_fallback_candidate(
        &mut self,
        sut: Noun,
        reference: Noun,
        hod: bool,
    ) -> Result<bool> {
        fn unwrapped_tag(ut: &mut Ut<'_>, typ: Noun, hod: bool) -> Result<TypeTagKind> {
            let space = ut.slab.noun_space();
            match type_tag_kind(typ, &space)? {
                TypeTagKind::Face => unwrapped_tag(ut, type_face_inner(typ, &space)?, hod),
                TypeTagKind::Hint => unwrapped_tag(ut, type_hint_inner(typ, &space)?, hod),
                TypeTagKind::Hold if hod => {
                    let repo = ut.repo(typ)?;
                    unwrapped_tag(ut, repo, hod)
                }
                kind => Ok(kind),
            }
        }

        let sut_tag = unwrapped_tag(self, sut, hod)?;
        let ref_tag = unwrapped_tag(self, reference, hod)?;
        Ok(matches!(
            (sut_tag, ref_tag),
            (TypeTagKind::Cell, TypeTagKind::Cell)
                | (TypeTagKind::Atom, TypeTagKind::Atom)
                | (TypeTagKind::Core, TypeTagKind::Core)
        ))
    }

    fn fire_wet_rib_contains(&mut self, sut: Noun, dox: Noun, hoon_noun: Noun) -> Result<bool> {
        if self.fire_wet_rib_raw.contains(&(
            unsafe { sut.as_raw() },
            unsafe { dox.as_raw() },
            unsafe { hoon_noun.as_raw() },
        )) {
            return Ok(true);
        }
        let space = self.slab.noun_space();
        for (entry_sut, entry_dox, entry_hoon) in self.fire_wet_rib.iter() {
            if (unsafe { entry_sut.as_raw() } == unsafe { sut.as_raw() }
                || noun_eq(*entry_sut, sut, &space)?)
                && (unsafe { entry_dox.as_raw() } == unsafe { dox.as_raw() }
                    || noun_eq(*entry_dox, dox, &space)?)
                && (unsafe { entry_hoon.as_raw() } == unsafe { hoon_noun.as_raw() }
                    || noun_eq(*entry_hoon, hoon_noun, &space)?)
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(super) fn mull_check_wet(
        &mut self,
        wet_core: Noun,
        dox: Noun,
        hoon_noun: Noun,
    ) -> Result<()> {
        if !self.vet {
            return Ok(());
        }
        let hoon_identity = self.canonicalize_nonsemantic_hoon_noun(hoon_noun);
        if self.fire_wet_rib_contains(wet_core, dox, hoon_identity)? {
            return Ok(());
        }
        self.fire_wet_rib_raw.insert((
            unsafe { wet_core.as_raw() },
            unsafe { dox.as_raw() },
            unsafe { hoon_identity.as_raw() },
        ));
        self.fire_wet_rib.push((wet_core, dox, hoon_identity));
        let result = (|| -> Result<()> {
            let hoon_ast = self.hoon_ast_lookup_result(hoon_noun).map_err(|err| {
                CompilerError::UnsupportedExpr(format!(
                    "native mint: fire-wet arm ast missing: {err}"
                ))
            })?;
            let noun_goal = ty_noun(self.slab);
            let _ = self.mull(wet_core, noun_goal, dox, hoon_ast.as_ref())?;
            Ok(())
        })();
        if let Some((entry_sut, entry_dox, entry_hoon)) = self.fire_wet_rib.pop() {
            self.fire_wet_rib_raw.remove(&(
                unsafe { entry_sut.as_raw() },
                unsafe { entry_dox.as_raw() },
                unsafe { entry_hoon.as_raw() },
            ));
        }
        result
    }

    fn redo_dear(&mut self, state: &RedoState) -> Result<Option<Vec<Noun>>> {
        if state.wec.len() != 1 {
            return Ok(None);
        }
        Ok(Some(
            self.redo_merge_face_stacks(&state.hos, &state.wec[0])?,
        ))
    }

    fn redo_merge_face_stacks(
        &mut self,
        subject_faces: &[Noun],
        reference_faces: &[Noun],
    ) -> Result<Vec<Noun>> {
        let space = self.slab.noun_space();
        let mut overlap = 0usize;
        let max_overlap = cmp::min(subject_faces.len(), reference_faces.len());
        for candidate in 0..=max_overlap {
            let start = subject_faces.len().saturating_sub(candidate);
            let mut matches = true;
            for idx in 0..candidate {
                if !noun_eq(subject_faces[start + idx], reference_faces[idx], &space)? {
                    matches = false;
                    break;
                }
            }
            if matches {
                overlap = candidate;
            }
        }
        let mut merged = subject_faces.to_vec();
        merged.extend_from_slice(&reference_faces[overlap..]);
        Ok(merged)
    }

    fn redo_done(&mut self, sut: Noun, state: &RedoState) -> Result<Noun> {
        let Some(lov) = self.redo_dear(state)? else {
            return Err(CompilerError::Noun("redo-match".to_string()));
        };
        let mut out = sut;
        for tool in lov.iter().rev() {
            out = ty_face_tool(self.slab, *tool, out);
        }
        Ok(out)
    }

    fn redo_push_unique_face_stack(
        &mut self,
        stacks: &mut Vec<Vec<Noun>>,
        candidate: Vec<Noun>,
    ) -> Result<()> {
        let space = self.slab.noun_space();
        for existing in stacks.iter() {
            if existing.len() != candidate.len() {
                continue;
            }
            let mut same = true;
            for (left, right) in existing.iter().zip(candidate.iter()) {
                if !noun_eq(*left, *right, &space)? {
                    same = false;
                    break;
                }
            }
            if same {
                return Ok(());
            }
        }
        stacks.push(candidate);
        Ok(())
    }

    fn redo_gil_contains(
        &mut self,
        gil: &[(Noun, Noun)],
        sut: Noun,
        reference: Noun,
    ) -> Result<bool> {
        let space = self.slab.noun_space();
        for (prior_sut, prior_ref) in gil {
            if (unsafe { prior_sut.raw_equals(&sut) } || noun_eq(*prior_sut, sut, &space)?)
                && (unsafe { prior_ref.raw_equals(&reference) }
                    || noun_eq(*prior_ref, reference, &space)?)
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn redo_subject_hold_in_fan(&mut self, hold: Noun) -> Result<bool> {
        let space = self.slab.noun_space();
        let (inner, hoon) = type_hold_parts(hold, &space)?;
        let leg_id = self.hold_repo_fan_leg_id_for_hold_type(hold, inner, hoon)?;
        Ok(self
            .hold_repo_fan_active_leg_ids
            .binary_search(&leg_id)
            .is_ok())
    }

    pub(super) fn redo_wet_payload(&mut self, payload: Noun, reference: Noun) -> Result<Noun> {
        if let Some(cached) = self.redo_boundary_lookup(payload, reference)? {
            return Ok(cached);
        }
        let result = self.redo_dext(payload, reference, RedoState::default())?;
        self.redo_boundary_store(payload, reference, result)?;
        Ok(result)
    }

    fn redo_dext(&mut self, sut: Noun, reference: Noun, state: RedoState) -> Result<Noun> {
        self.with_stack_guard(|ut| ut.redo_dext_impl(sut, reference, state))
    }

    fn redo_dext_impl(&mut self, sut: Noun, reference: Noun, state: RedoState) -> Result<Noun> {
        let space = self.slab.noun_space();
        if noun_eq(sut, reference, &space)?
            || matches!(
                type_tag_kind(reference, &space)?,
                TypeTagKind::Noun | TypeTagKind::Void | TypeTagKind::Atom | TypeTagKind::Core
            )
        {
            return self.redo_done(sut, &state);
        }

        match type_tag_kind(sut, &space)? {
            TypeTagKind::Noun | TypeTagKind::Void | TypeTagKind::Atom | TypeTagKind::Core => {
                let (_reduced_ref, next_state) = self.redo_sint(sut, reference, true, state)?;
                self.redo_done(sut, &next_state)
            }
            TypeTagKind::Cell => {
                let (reduced_ref, next_state) = self.redo_sint(sut, reference, true, state)?;
                let (sut_head, sut_tail) = type_cell_parts(sut, &space)?;
                let descend_state = next_state.for_cell_descent();
                let ref_head = self.peek(reduced_ref, Way::Free, 2)?;
                let ref_tail = self.peek(reduced_ref, Way::Free, 3)?;
                let new_head = self.redo_dext(sut_head, ref_head, descend_state.clone())?;
                let new_tail = self.redo_dext(sut_tail, ref_tail, descend_state)?;
                let rebuilt = ty_cell(self.slab, new_head, new_tail);
                self.redo_done(rebuilt, &next_state)
            }
            TypeTagKind::Face => {
                let mut next_state = state;
                next_state.hos.push(type_face_tool(sut, &space)?);
                self.redo_dext(type_face_inner(sut, &space)?, reference, next_state)
            }
            TypeTagKind::Hint => {
                let (inner, note, payload) = type_hint_parts(sut, &space)?;
                let redone = self.redo_dext(payload, reference, state)?;
                Ok(ty_hint(self.slab, inner, note, redone))
            }
            TypeTagKind::Fork => {
                let mut rebuilt = Vec::new();
                for option in type_fork_options(sut, &space)? {
                    rebuilt.push(self.redo_dext(option, reference, state.clone())?);
                }
                self.fork_from_options(rebuilt)
            }
            TypeTagKind::Hold => {
                let (reduced_ref, next_state) = self.redo_sint(sut, reference, false, state)?;
                if self.redo_subject_hold_in_fan(sut)? {
                    let (_expanded_ref, fan_state) =
                        self.redo_sint(sut, reduced_ref, true, next_state)?;
                    return self.redo_done(sut, &fan_state);
                }
                if self.redo_gil_contains(&next_state.gil, sut, reduced_ref)? {
                    return self.redo_done(sut, &next_state);
                }
                let repo = self.repo(sut)?;
                let mut recurse_state = next_state;
                recurse_state.gil.push((sut, reduced_ref));
                let redone = self.redo_dext(repo, reduced_ref, recurse_state)?;
                if noun_eq(redone, repo, &self.slab.noun_space())? {
                    Ok(sut)
                } else {
                    Ok(redone)
                }
            }
        }
    }

    pub(super) fn redo_sint(
        &mut self,
        sut: Noun,
        reference: Noun,
        hod: bool,
        state: RedoState,
    ) -> Result<(Noun, RedoState)> {
        self.with_stack_guard(|ut| ut.redo_sint_impl(sut, reference, hod, state))
    }

    fn redo_sint_impl(
        &mut self,
        sut: Noun,
        reference: Noun,
        hod: bool,
        state: RedoState,
    ) -> Result<(Noun, RedoState)> {
        let space = self.slab.noun_space();
        match type_tag_kind(reference, &space)? {
            TypeTagKind::Hint => {
                self.redo_sint(sut, type_hint_inner(reference, &space)?, hod, state)
            }
            TypeTagKind::Face => {
                let mut next_state = state;
                let tool = type_face_tool(reference, &space)?;
                for stack in next_state.wec.iter_mut() {
                    stack.push(tool);
                }
                self.redo_sint(sut, type_face_inner(reference, &space)?, hod, next_state)
            }
            TypeTagKind::Fork => {
                let options = type_fork_options(reference, &space)?;
                let mut merged_wec = Vec::new();
                let mut reduced_options = Vec::new();
                for option in options.iter().copied() {
                    if self.miss(sut, option)? {
                        continue;
                    }
                    let (reduced_option, branch_state) =
                        self.redo_sint(sut, option, hod, state.clone())?;
                    for stack in branch_state.wec {
                        self.redo_push_unique_face_stack(&mut merged_wec, stack)?;
                    }
                    reduced_options.push(reduced_option);
                }
                if merged_wec.is_empty() {
                    for option in options.iter().copied() {
                        if !self.redo_fork_fallback_candidate(sut, option, hod)? {
                            continue;
                        }
                        let (reduced_option, branch_state) =
                            self.redo_sint(sut, option, hod, state.clone())?;
                        for stack in branch_state.wec {
                            self.redo_push_unique_face_stack(&mut merged_wec, stack)?;
                        }
                        reduced_options.push(reduced_option);
                    }
                }
                let mut next_state = state;
                next_state.wec = merged_wec;
                Ok((self.fork_from_options(reduced_options)?, next_state))
            }
            TypeTagKind::Hold if hod => {
                let repo_ref = self.repo(reference)?;
                self.redo_sint(sut, repo_ref, hod, state)
            }
            _ => Ok((reference, state)),
        }
    }
}
