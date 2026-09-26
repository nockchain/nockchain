use super::*;

impl<'a> Ut<'a> {
    #[cfg(test)]
    fn collect_rest_leg_ids(&mut self, legs: &[(Noun, Noun)]) -> Result<Vec<FanLegId>> {
        let mut unique_leg_ids = Vec::new();
        for (inner, hoon_noun) in legs {
            let leg_id = self.hold_repo_fan_leg_intern_id(*inner, *hoon_noun)?;
            if !unique_leg_ids.contains(&leg_id) {
                unique_leg_ids.push(leg_id);
            }
        }
        Ok(unique_leg_ids)
    }

    fn with_active_rest_leg_ids<R>(
        &mut self,
        leg_ids: &[FanLegId],
        body: impl FnOnce(&mut Self) -> Result<R>,
    ) -> Result<R> {
        if leg_ids.iter().any(|leg_id| {
            self.hold_repo_fan_active_leg_ids
                .binary_search(leg_id)
                .is_ok()
        }) {
            return Err(CompilerError::Noun("rest-loop".to_string()));
        }

        for leg_id in leg_ids.iter().copied() {
            let inserted = self.hold_repo_fan_activate_leg_id(leg_id);
            debug_assert!(
                inserted,
                "rest leg id should only be activated once per scope"
            );
        }

        let result = body(self);

        for leg_id in leg_ids.iter().rev().copied() {
            self.hold_repo_fan_deactivate_leg_id(leg_id);
        }

        result
    }

    #[cfg(test)]
    pub(super) fn with_rest_legs<R>(
        &mut self,
        legs: &[(Noun, Noun)],
        body: impl FnOnce(&mut Self) -> Result<R>,
    ) -> Result<R> {
        if legs.is_empty() {
            return body(self);
        }
        let unique_leg_ids = self.collect_rest_leg_ids(legs)?;
        self.with_active_rest_leg_ids(&unique_leg_ids, body)
    }

    pub(super) fn with_rest_leg_id<R>(
        &mut self,
        leg_id: FanLegId,
        body: impl FnOnce(&mut Self) -> Result<R>,
    ) -> Result<R> {
        self.with_active_rest_leg_ids(&[leg_id], body)
    }

    /// Plays each leg's hoon against its native subject and forks the results.
    /// The fork is built on the noun path to keep hoon-138's mug ordering; the
    /// caller (`repo_hold`) lifts it to native.
    pub(super) fn rest_inner(&mut self, legs: &[(NRc<NTy>, Noun)]) -> Result<Noun> {
        let mut played = Vec::with_capacity(legs.len());
        for (inner, hoon_noun) in legs {
            let space = self.slab.noun_space();
            let hoon = self.hoon_ast_lookup_result(*hoon_noun).map_err(|err| {
                let tag = Self::hoon_noun_tag(*hoon_noun, &space)
                    .unwrap_or_else(|| "<unknown>".to_string());
                CompilerError::Noun(format!(
                    "native rest: hold ast missing tag={tag} decode_err={err}"
                ))
            })?;
            let play_ty = self.play(inner.clone(), hoon.as_ref())?;
            played.push(live_to_noun(&mut self.cx, &play_ty, self.slab));
        }
        self.fork_from_options(played)
    }

    // HOON138:arm=ut:rest lines=10765-10775 map=direct status=partial reviewed=2026-03-06
    #[cfg(test)]
    // HOON138_NOTE:native direct helper for canonical `++rest`; cache policy still wraps this path
    pub(super) fn rest(&mut self, sut: Noun, legs: &[(Noun, Noun)]) -> Result<Noun> {
        let legs_noun = self.rest_legs_noun(legs);
        // The legs are nouns here; lift each inner subject to native for
        // `rest_inner`.
        let mut native_legs = Vec::with_capacity(legs.len());
        for (inner, hoon_noun) in legs {
            native_legs.push((
                native_of(&mut self.cx, *inner, &self.slab.noun_space())?,
                *hoon_noun,
            ));
        }
        self.with_rest_legs(legs, |ut| {
            if let Some(cached) = ut.rest_boundary_lookup(sut, legs_noun)? {
                return Ok(cached);
            }
            let result = ut.rest_inner(&native_legs)?;
            ut.rest_boundary_store(sut, legs_noun, result)?;
            Ok(result)
        })
    }

    // HOON138:arm=ut:rest lines=10765-10775 map=wrapper status=partial reviewed=2026-03-06
    #[cfg(test)]
    // HOON138_NOTE:scoped native helper for canonical `++rest` fan activation and loop checks
    pub(super) fn with_rest_leg<R>(
        &mut self,
        inner: Noun,
        hoon_noun: Noun,
        body: impl FnOnce(&mut Self) -> Result<R>,
    ) -> Result<R> {
        let leg = [(inner, hoon_noun)];
        self.with_rest_legs(&leg, body)
    }

    fn repo_hold(
        &mut self,
        typ: Noun,
        subject: NRc<NTy>,
        inner: Noun,
        hoon_noun: Noun,
    ) -> Result<NRc<NTy>> {
        // `subject` is the leg's native inner type, passed to `play` through
        // `rest_inner`. The noun `inner` (`live_to_noun(subject)`) is used only for
        // the noun-keyed leg-id intern and `rest_boundary` cache.
        let native_legs = [(subject, hoon_noun)];
        let leg_id = self.hold_repo_fan_leg_id_for_hold_type(typ, inner, hoon_noun)?;
        let legs_noun = self.rest_legs_noun(&[(inner, hoon_noun)]);
        let result_noun = self.with_rest_leg_id(leg_id, |ut| {
            if let Some(cached) = ut.rest_boundary_lookup(typ, legs_noun)? {
                if ut.memo_verify.due(MemoSite::Rest) {
                    let fresh = ut.memo_verify_recompute(MemoSite::Rest, false, |ut| {
                        ut.rest_inner(&native_legs)
                    });
                    let matched = matches!(fresh, Ok(noun) if ut.memo_verify_noun_eq(noun, cached));
                    verify::record(MemoSite::Rest, matched, || {
                        format!("recomputed ok: {}", fresh.is_ok())
                    });
                }
                return Ok(cached);
            }
            let result = ut.rest_inner(&native_legs)?;
            ut.rest_boundary_store(typ, legs_noun, result)?;
            Ok(result)
        })?;
        // repo results are freshly built each recursion level; content-key the
        // decode so structurally equal expansions reuse one interned `Rc`.
        self.native_of_cached(result_noun)
    }

    pub(super) fn ty_hold_cached(&mut self, inner: Noun, hoon: Noun) -> Result<Noun> {
        let raw_key = HoldKey {
            subject: NounIdentity::of(inner),
            gene: NounIdentity::of(hoon),
        };
        if let Some(cached) = self.hold_memo.hold_type_raw.get(&raw_key) {
            return Ok(cached);
        }

        let space = self.slab.noun_space();
        let key = HoldKey {
            subject: self.noun_mug_cached(inner),
            gene: self.noun_mug_cached(hoon),
        };
        if let Some(entries) = self.hold_memo.hold_type.get(&key) {
            for entry in entries.iter().rev() {
                let inner_match = unsafe { entry.inner.raw_equals(&inner) }
                    || noun_eq(entry.inner, inner, &space)?;
                if !inner_match {
                    continue;
                }
                let hoon_match =
                    unsafe { entry.hoon.raw_equals(&hoon) } || noun_eq(entry.hoon, hoon, &space)?;
                if hoon_match {
                    self.hold_memo.hold_type_raw.insert_with_limit(
                        raw_key,
                        entry.hold,
                        Self::HOLD_TYPE_CACHE_RAW_KEY_LIMIT,
                    );
                    return Ok(entry.hold);
                }
            }
        }

        let hold = ty_hold(self.slab, inner, hoon);
        self.hold_memo.hold_type_raw.insert_with_limit(
            raw_key,
            hold,
            Self::HOLD_TYPE_CACHE_RAW_KEY_LIMIT,
        );
        let bucket = self
            .hold_memo
            .hold_type
            .ensure_key(key, Self::HOLD_TYPE_CACHE_KEY_LIMIT);
        if bucket.len() >= Self::HOLD_TYPE_CACHE_BUCKET_LIMIT {
            bucket.pop_front();
        }
        bucket.push_back(HoldTypeCacheEntry { inner, hoon, hold });
        Ok(hold)
    }

    // HOON138:arm=ut:repo lines=10754-10763 map=direct status=partial reviewed=2026-03-06
    // HOON138_NOTE:native primary implementation for canonical `++repo`; full parity review is still in progress
    pub(super) fn repo(&mut self, typ: NRc<NTy>) -> Result<NRc<NTy>> {
        // `cons_cell` applies the same void collapse as the noun `cell_type`. The
        // %noun fork is built with the noun `fork_from_options` to keep its mug
        // ordering, then lifted back to native.
        match &*typ {
            NTy::Face { inner, .. } => Ok(inner.clone()),
            NTy::Hint { payload, .. } => Ok(payload.clone()),
            NTy::Core { payload, .. } => {
                let head = cons_noun(&mut self.cx);
                Ok(cons_cell(&mut self.cx, head, payload.clone()))
            }
            NTy::Hold { subject, gene } => {
                let subject = subject.clone();
                let gene = gene.clone();
                // `inner` and `typ_noun` are lowered only for the noun-keyed leg-id
                // and `rest_boundary` cache; `play` gets the native `subject`.
                let inner = live_to_noun(&mut self.cx, &subject, self.slab);
                let hoon = live_leaf_to_noun(&mut self.cx, &gene, self.slab);
                let typ_noun = live_to_noun(&mut self.cx, &typ, self.slab);
                self.repo_hold(typ_noun, subject, inner, hoon)
            }
            NTy::Noun => {
                let atom = ty_atom(self.slab, "$", None);
                let noun = ty_noun(self.slab);
                let cell = ty_cell(self.slab, noun, noun);
                let fork_noun = self.fork_from_options(vec![atom, cell])?;
                self.native_of_cached(fork_noun)
            }
            _ => Err(CompilerError::Noun("repo-fltt".to_string())),
        }
    }

    /// `repo` for a noun type: lifts it to native, runs `repo`, and lowers the
    /// result.
    pub(super) fn repo_noun(&mut self, typ: Noun) -> Result<Noun> {
        // Content-keyed decode, so structurally equal nouns reuse one interned `Rc`.
        let native = self.native_of_cached(typ)?;
        let r = self.repo(native)?;
        Ok(live_to_noun(&mut self.cx, &r, self.slab))
    }
}
