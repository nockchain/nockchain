use super::*;

#[derive(Clone)]
pub(super) struct RedoState {
    /// Face tools collected while descending through subject `%face`s.
    hos: Vec<NLeaf>,
    /// Reference face-tool stacks, one per surviving fork branch, pushed by
    /// `redo_sint` as it descends through reference `%face`s.
    wec: Vec<Vec<NLeaf>>,
    /// %hold recursion-cut set of visited `(subject, reference)` pairs, compared
    /// by pointer. Every redo-produced type is interned (`cons_*`, `repo`,
    /// `peek`, `native_of`), so pointer equality is structural equality.
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
    fn fire_wet_rib_contains(
        &mut self,
        sut: &NRc<NTy>,
        dox: &NRc<NTy>,
        hoon_noun: Noun,
    ) -> Result<bool> {
        if self.fire_wet_rib_raw.contains(&WetRibKey {
            subject: native_type_id(sut),
            secondary_subject: native_type_id(dox),
            gene: NounIdentity::of(hoon_noun),
        }) {
            return Ok(true);
        }
        let space = self.slab.noun_space();
        for (entry_sut, entry_dox, entry_hoon) in self.fire_wet_rib.iter() {
            // sut and dox are interned, so pointer equality is structural equality.
            if NRc::ptr_eq(entry_sut, sut)
                && NRc::ptr_eq(entry_dox, dox)
                && (unsafe { entry_hoon.raw_equals(&hoon_noun) }
                    || noun_eq(*entry_hoon, hoon_noun, &space)?)
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// The vet-time half of hoon-138 `++fire` for a wet arm: unless
    /// `[sut dox arm]` is already in `rib`, mull the arm against the redone
    /// core with that key added. `sut` is the call-site subject, not the
    /// redone core, so a re-entry from an equal subject is cut even when its
    /// redone core differs.
    pub(super) fn mull_check_wet(
        &mut self,
        sut: &NRc<NTy>,
        wet_core: NRc<NTy>,
        dox: NRc<NTy>,
        hoon_noun: Noun,
    ) -> Result<()> {
        if !self.vet {
            return Ok(());
        }
        let hoon_identity = self.canonicalize_nonsemantic_hoon_noun(hoon_noun);
        if self.fire_wet_rib_contains(sut, &dox, hoon_identity)? {
            return Ok(());
        }
        self.fire_wet_rib_raw.insert(WetRibKey {
            subject: native_type_id(sut),
            secondary_subject: native_type_id(&dox),
            gene: NounIdentity::of(hoon_identity),
        });
        self.fire_wet_rib
            .push((sut.clone(), dox.clone(), hoon_identity));
        let result = (|| -> Result<()> {
            let hoon_ast = self.hoon_ast_lookup_result(hoon_noun).map_err(|err| {
                CompilerError::UnsupportedExpr(format!(
                    "native mint: fire-wet arm ast missing: {err}"
                ))
            })?;
            let noun_goal_n = cons_noun(&mut self.cx);
            let _ = self.mull(wet_core, noun_goal_n, dox, hoon_ast.as_ref())?;
            Ok(())
        })();
        if let Some((entry_sut, entry_dox, entry_hoon)) = self.fire_wet_rib.pop() {
            self.fire_wet_rib_raw.remove(&WetRibKey {
                subject: native_type_id(&entry_sut),
                secondary_subject: native_type_id(&entry_dox),
                gene: NounIdentity::of(entry_hoon),
            });
        }
        result
    }

    fn redo_dear(&mut self, state: &RedoState) -> Result<Option<Vec<NLeaf>>> {
        // hoon-138 `++dear`: an empty `wec` (every reference fork case pruned
        // by `++sint`) implies void and yields no faces at all, not even the
        // subject's; more than one reference face stack is a redo-match.
        if state.wec.is_empty() {
            return Ok(Some(Vec::new()));
        }
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
        // hoon-138 `++dear` builds `(weld hos (slag lip har))` in innermost-first
        // order and `++done` wraps from the leaf outward, so reference faces nest
        // outside the subject's. These stacks are outermost-first (pushed on
        // descent) and `redo_done` wraps in reverse, so the equivalent merge is
        // reference ++ subject, minus the longest overlap where the subject's
        // outermost faces equal the reference's innermost (subject prefix ==
        // reference suffix).
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
        // The leg-id intern is noun-keyed, so lower the hold, its subject, and its
        // gene only to compute the key.
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
            if self.memo_verify.due(MemoSite::Redo) {
                let fresh = self.memo_verify_recompute(MemoSite::Redo, false, |ut| {
                    let payload_n = ut.native_of_cached(payload)?;
                    let reference_n = ut.native_of_cached(reference)?;
                    let result_n = ut.redo_dext(payload_n, reference_n, RedoState::default())?;
                    Ok(live_to_noun(&mut ut.cx, &result_n, ut.slab))
                });
                let matched = matches!(fresh, Ok(noun) if self.memo_verify_noun_eq(noun, cached));
                verify::record(MemoSite::Redo, matched, || {
                    format!("recomputed ok: {}", fresh.is_ok())
                });
            }
            return Ok(cached);
        }
        // Decode payload and reference once, run the native redo, and lower the
        // result once. The `redo_boundary` cache is noun-keyed.
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
                let ref_head = self.peek(reduced_ref.clone(), Way::Free, 2u64)?;
                let ref_tail = self.peek(reduced_ref, Way::Free, 3u64)?;
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
                // Recursion cut (the `gil` check in hoon-138 `++redo:dext`). The
                // post-`sint` reference can change at every level, so `gil` records
                // both it and the original reference. A repeated (sut, ref) pair
                // returns the unchanged %hold, `play` reproduces the same hold type,
                // and the fork set collapses it (matching `++rest`).
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
