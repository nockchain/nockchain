use super::*;

#[derive(Clone)]
pub(super) struct RedoState {
    /// Subject-side face TOOLS accumulated descending through `%face`s on the
    /// SUBJECT side (`NTy::Face.tool`).
    hos: Vec<NLeaf>,
    /// Reference-side face TOOL stacks (one stack per surviving fork branch),
    /// accumulated by `redo_sint` descending through `%face`s on the REFERENCE.
    wec: Vec<Vec<NLeaf>>,
    /// The %hold recursion-cut set: visited `(subject, reference)` pairs, by
    /// interned `Rc` identity (every redo-produced type is interned via
    /// `cons_*`/`repo`/`peek`/`native_of`, so structural equality == pointer
    /// equality).
    gil: Vec<(NRc<NTy>, NRc<NTy>)>,
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
        sut: &NRc<NTy>,
        reference: &NRc<NTy>,
        hod: bool,
    ) -> Result<bool> {
        // Strip %face/%hint (and %hold under `hod`) wrappers to reach the base
        // tag, then accept only matching base kinds (cell/atom/core). Mirrors the
        // old noun `unwrapped_tag`, reading the native enum directly.
        #[derive(PartialEq, Clone, Copy)]
        enum Base {
            Cell,
            Atom,
            Core,
            Other,
        }
        fn unwrapped(ut: &mut Ut<'_>, typ: &NRc<NTy>, hod: bool) -> Result<Base> {
            match &**typ {
                NTy::Face { inner, .. } => unwrapped(ut, &inner.clone(), hod),
                NTy::Hint { payload, .. } => unwrapped(ut, &payload.clone(), hod),
                NTy::Hold { .. } if hod => {
                    let repo = ut.repo(typ.clone())?;
                    unwrapped(ut, &repo, hod)
                }
                NTy::Cell(..) => Ok(Base::Cell),
                NTy::Atom { .. } => Ok(Base::Atom),
                NTy::Core { .. } => Ok(Base::Core),
                _ => Ok(Base::Other),
            }
        }

        let sut_tag = unwrapped(self, sut, hod)?;
        let ref_tag = unwrapped(self, reference, hod)?;
        Ok(matches!(
            (sut_tag, ref_tag),
            (Base::Cell, Base::Cell) | (Base::Atom, Base::Atom) | (Base::Core, Base::Core)
        ))
    }

    fn fire_wet_rib_contains(
        &mut self,
        sut: &NRc<NTy>,
        dox: &NRc<NTy>,
        hoon_noun: Noun,
    ) -> Result<bool> {
        if self.fire_wet_rib_raw.contains(&(
            NRc::as_ptr(sut) as usize,
            NRc::as_ptr(dox) as usize,
            unsafe { hoon_noun.as_raw() },
        )) {
            return Ok(true);
        }
        let space = self.slab.noun_space();
        for (entry_sut, entry_dox, entry_hoon) in self.fire_wet_rib.iter() {
            // sut/dox are interned natives: ptr-identity IS structural identity.
            if NRc::ptr_eq(entry_sut, sut)
                && NRc::ptr_eq(entry_dox, dox)
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
        wet_core: NRc<NTy>,
        dox: NRc<NTy>,
        hoon_noun: Noun,
    ) -> Result<()> {
        if !self.vet {
            return Ok(());
        }
        let hoon_identity = self.canonicalize_nonsemantic_hoon_noun(hoon_noun);
        if self.fire_wet_rib_contains(&wet_core, &dox, hoon_identity)? {
            return Ok(());
        }
        self.fire_wet_rib_raw.insert((
            NRc::as_ptr(&wet_core) as usize,
            NRc::as_ptr(&dox) as usize,
            unsafe { hoon_identity.as_raw() },
        ));
        self.fire_wet_rib
            .push((wet_core.clone(), dox.clone(), hoon_identity));
        let result = (|| -> Result<()> {
            let hoon_ast = self.hoon_ast_lookup_result(hoon_noun).map_err(|err| {
                CompilerError::UnsupportedExpr(format!(
                    "native mint: fire-wet arm ast missing: {err}"
                ))
            })?;
            // mull is native (C7); wet_core/dox are already native.
            let noun_goal_n = cons_noun(&mut self.cx);
            let _ = self.mull(wet_core, noun_goal_n, dox, hoon_ast.as_ref())?;
            Ok(())
        })();
        if let Some((entry_sut, entry_dox, entry_hoon)) = self.fire_wet_rib.pop() {
            self.fire_wet_rib_raw.remove(&(
                NRc::as_ptr(&entry_sut) as usize,
                NRc::as_ptr(&entry_dox) as usize,
                unsafe { entry_hoon.as_raw() },
            ));
        }
        result
    }

    fn redo_dear(&mut self, state: &RedoState) -> Result<Option<Vec<NLeaf>>> {
        if state.wec.len() != 1 {
            return Ok(None);
        }
        Ok(Some(
            self.redo_merge_face_stacks(&state.hos, &state.wec[0])?,
        ))
    }

    fn redo_merge_face_stacks(
        &mut self,
        subject_faces: &[NLeaf],
        reference_faces: &[NLeaf],
    ) -> Result<Vec<NLeaf>> {
        // hoon-138 ++dear: `(weld hos (slag lip har))` in innermost-first stack
        // order, applied leaf-outward by ++done — REFERENCE faces nest OUTSIDE
        // the subject's own. Our stacks are outermost-first (push on descent)
        // and redo_done wraps with .rev() (last element innermost), so the
        // equivalent forward merge is reference ++ subject, dropping the
        // overlap where the subject's outermost faces coincide with the
        // reference's innermost (subject prefix == reference suffix here).
        let mut overlap = 0usize;
        let max_overlap = cmp::min(subject_faces.len(), reference_faces.len());
        for candidate in 0..=max_overlap {
            let start = reference_faces.len() - candidate;
            let mut matches = true;
            for idx in 0..candidate {
                if reference_faces[start + idx] != subject_faces[idx] {
                    matches = false;
                    break;
                }
            }
            if matches {
                overlap = candidate;
            }
        }
        let mut merged = reference_faces[..reference_faces.len() - overlap].to_vec();
        merged.extend_from_slice(subject_faces);
        Ok(merged)
    }

    fn redo_done(&mut self, sut: NRc<NTy>, state: &RedoState) -> Result<NRc<NTy>> {
        let Some(lov) = self.redo_dear(state)? else {
            return Err(CompilerError::Noun("redo-match".to_string()));
        };
        let mut out = sut;
        for tool in lov.iter().rev() {
            out = cons_face(&mut self.cx, tool.clone(), out);
        }
        Ok(out)
    }

    fn redo_push_unique_face_stack(
        &mut self,
        stacks: &mut Vec<Vec<NLeaf>>,
        candidate: Vec<NLeaf>,
    ) -> Result<()> {
        for existing in stacks.iter() {
            if existing.len() != candidate.len() {
                continue;
            }
            if existing.iter().zip(candidate.iter()).all(|(l, r)| l == r) {
                return Ok(());
            }
        }
        stacks.push(candidate);
        Ok(())
    }

    fn redo_gil_contains(
        &mut self,
        gil: &[(NRc<NTy>, NRc<NTy>)],
        sut: &NRc<NTy>,
        reference: &NRc<NTy>,
    ) -> Result<bool> {
        for (prior_sut, prior_ref) in gil {
            if NRc::ptr_eq(prior_sut, sut) && NRc::ptr_eq(prior_ref, reference) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn redo_subject_hold_in_fan(&mut self, hold: &NRc<NTy>) -> Result<bool> {
        // The leg-id intern is still noun-keyed (deferred re-key): lower the
        // hold + its inner subject / gene gate ONLY to compute the leg-id key.
        let NTy::Hold { subject, gene } = &**hold else {
            return Ok(false);
        };
        let inner = live_to_noun(&mut self.cx, subject, self.slab);
        let hoon = live_leaf_to_noun(&mut self.cx, gene, self.slab);
        let hold_noun = live_to_noun(&mut self.cx, hold, self.slab);
        let leg_id = self.hold_repo_fan_leg_id_for_hold_type(hold_noun, inner, hoon)?;
        Ok(self
            .hold_repo_fan_active_leg_ids
            .binary_search(&leg_id)
            .is_ok())
    }

    pub(super) fn redo_wet_payload(&mut self, payload: Noun, reference: Noun) -> Result<Noun> {
        if let Some(cached) = self.redo_boundary_lookup(payload, reference)? {
            return Ok(cached);
        }
        // BOUNDARY: decode payload + reference to native ONCE here; the per-level
        // redo recursion is entirely native (no Type<->Noun round-trips inside);
        // lower the result ONCE on the way out. The redo_boundary cache stays
        // noun-keyed at this entry.
        let payload_n = self.native_of_cached(payload)?;
        let reference_n = self.native_of_cached(reference)?;
        let result_n = self.redo_dext(payload_n, reference_n, RedoState::default())?;
        let result = live_to_noun(&mut self.cx, &result_n, self.slab);
        self.redo_boundary_store(payload, reference, result)?;
        Ok(result)
    }

    fn redo_dext(
        &mut self,
        sut: NRc<NTy>,
        reference: NRc<NTy>,
        state: RedoState,
    ) -> Result<NRc<NTy>> {
        self.with_stack_guard(|ut| ut.redo_dext_impl(sut, reference, state))
    }

    fn redo_dext_impl(
        &mut self,
        sut: NRc<NTy>,
        reference: NRc<NTy>,
        state: RedoState,
    ) -> Result<NRc<NTy>> {
        if NRc::ptr_eq(&sut, &reference)
            || matches!(
                &*reference,
                NTy::Noun | NTy::Void | NTy::Atom { .. } | NTy::Core { .. }
            )
        {
            return self.redo_done(sut, &state);
        }

        match &*sut {
            NTy::Noun | NTy::Void | NTy::Atom { .. } | NTy::Core { .. } => {
                let (_reduced_ref, next_state) =
                    self.redo_sint(sut.clone(), reference, true, state)?;
                self.redo_done(sut, &next_state)
            }
            NTy::Cell(sut_head, sut_tail) => {
                let sut_head = sut_head.clone();
                let sut_tail = sut_tail.clone();
                let (reduced_ref, next_state) =
                    self.redo_sint(sut.clone(), reference, true, state)?;
                let descend_state = next_state.for_cell_descent();
                let ref_head = self.peek(reduced_ref.clone(), Way::Free, 2)?;
                let ref_tail = self.peek(reduced_ref, Way::Free, 3)?;
                let new_head = self.redo_dext(sut_head, ref_head, descend_state.clone())?;
                let new_tail = self.redo_dext(sut_tail, ref_tail, descend_state)?;
                let rebuilt = cons_cell(&mut self.cx, new_head, new_tail);
                self.redo_done(rebuilt, &next_state)
            }
            NTy::Face { tool, inner } => {
                let tool = tool.clone();
                let inner = inner.clone();
                let mut next_state = state;
                next_state.hos.push(tool);
                self.redo_dext(inner, reference, next_state)
            }
            NTy::Hint { head, payload } => {
                let head = head.clone();
                let payload = payload.clone();
                let redone = self.redo_dext(payload, reference, state)?;
                Ok(cons_hint(&mut self.cx, head, redone))
            }
            NTy::Fork { .. } => {
                let options = self.fork_options_native(&sut)?;
                let mut rebuilt = Vec::with_capacity(options.len());
                for option in options {
                    rebuilt.push(self.redo_dext(option, reference.clone(), state.clone())?);
                }
                self.cons_fork(rebuilt)
            }
            NTy::Hold { .. } => {
                let (reduced_ref, next_state) =
                    self.redo_sint(sut.clone(), reference.clone(), false, state)?;
                if self.redo_subject_hold_in_fan(&sut)? {
                    let (_expanded_ref, fan_state) =
                        self.redo_sint(sut.clone(), reduced_ref, true, next_state)?;
                    return self.redo_done(sut, &fan_state);
                }
                // RT-05 recursion-cut: hoon-138 ++redo:dext keys its %hold loop set
                // on the ORIGINAL reference ([sut ref]); honk only tracked the
                // post-`sint` `reduced_ref`, which changes every level, so the
                // recursive back-edge never matched and the hold unrolled forever.
                // Track BOTH so a repeated (sut, ref) closes the cycle: the cut
                // returns the unchanged %hold, `play` reproduces the identical hold
                // type, and the fork mug-set collapses it (matching ++rest).
                // Identity is by interned `Rc` pointer: every redo-produced type is
                // interned via cons_*/repo/peek/native_of, so ptr_eq == structural
                // equality.
                if self.redo_gil_contains(&next_state.gil, &sut, &reduced_ref)?
                    || self.redo_gil_contains(&next_state.gil, &sut, &reference)?
                {
                    return self.redo_done(sut, &next_state);
                }
                let repo = self.repo(sut.clone())?;
                let mut recurse_state = next_state;
                recurse_state.gil.push((sut.clone(), reduced_ref.clone()));
                recurse_state.gil.push((sut.clone(), reference));
                let redone = self.redo_dext(repo.clone(), reduced_ref, recurse_state)?;
                if NRc::ptr_eq(&redone, &repo) {
                    Ok(sut)
                } else {
                    Ok(redone)
                }
            }
        }
    }

    pub(super) fn redo_sint(
        &mut self,
        sut: NRc<NTy>,
        reference: NRc<NTy>,
        hod: bool,
        state: RedoState,
    ) -> Result<(NRc<NTy>, RedoState)> {
        self.with_stack_guard(|ut| ut.redo_sint_impl(sut, reference, hod, state))
    }

    fn redo_sint_impl(
        &mut self,
        sut: NRc<NTy>,
        reference: NRc<NTy>,
        hod: bool,
        state: RedoState,
    ) -> Result<(NRc<NTy>, RedoState)> {
        match &*reference {
            NTy::Hint { payload, .. } => {
                let payload = payload.clone();
                self.redo_sint(sut, payload, hod, state)
            }
            NTy::Face { tool, inner } => {
                let tool = tool.clone();
                let inner = inner.clone();
                let mut next_state = state;
                for stack in next_state.wec.iter_mut() {
                    stack.push(tool.clone());
                }
                self.redo_sint(sut, inner, hod, next_state)
            }
            NTy::Fork { .. } => {
                let options = self.fork_options_native(&reference)?;
                let mut merged_wec = Vec::new();
                let mut reduced_options = Vec::new();
                for option in options.iter() {
                    if self.miss(sut.clone(), option.clone())? {
                        continue;
                    }
                    let (reduced_option, branch_state) =
                        self.redo_sint(sut.clone(), option.clone(), hod, state.clone())?;
                    for stack in branch_state.wec {
                        self.redo_push_unique_face_stack(&mut merged_wec, stack)?;
                    }
                    reduced_options.push(reduced_option);
                }
                if merged_wec.is_empty() {
                    for option in options.iter() {
                        if !self.redo_fork_fallback_candidate(&sut, option, hod)? {
                            continue;
                        }
                        let (reduced_option, branch_state) =
                            self.redo_sint(sut.clone(), option.clone(), hod, state.clone())?;
                        for stack in branch_state.wec {
                            self.redo_push_unique_face_stack(&mut merged_wec, stack)?;
                        }
                        reduced_options.push(reduced_option);
                    }
                }
                let mut next_state = state;
                next_state.wec = merged_wec;
                Ok((self.cons_fork(reduced_options)?, next_state))
            }
            NTy::Hold { .. } if hod => {
                let repo_ref = self.repo(reference)?;
                self.redo_sint(sut, repo_ref, hod, state)
            }
            _ => Ok((reference, state)),
        }
    }
}
