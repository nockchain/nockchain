use super::*;

impl<'a> Ut<'a> {
    #[cfg(test)]
    fn collect_rest_leg_ids(&mut self, legs: &[(Noun, Noun)]) -> Result<Vec<u64>> {
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
        leg_ids: &[u64],
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
        leg_id: u64,
        body: impl FnOnce(&mut Self) -> Result<R>,
    ) -> Result<R> {
        self.with_active_rest_leg_ids(&[leg_id], body)
    }

    pub(super) fn rest_inner(&mut self, legs: &[(Noun, Noun)]) -> Result<Noun> {
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
            played.push(self.play(*inner, hoon.as_ref())?);
        }
        self.fork_from_options(played)
    }

    // HOON138:arm=ut:rest lines=10765-10775 map=direct status=partial reviewed=2026-03-06
    #[cfg(test)]
    // HOON138_NOTE:native direct helper for canonical `++rest`; cache policy still wraps this path
    pub(super) fn rest(&mut self, sut: Noun, legs: &[(Noun, Noun)]) -> Result<Noun> {
        let legs_noun = self.rest_legs_noun(legs);
        self.with_rest_legs(legs, |ut| {
            if let Some(cached) = ut.rest_boundary_lookup(sut, legs_noun)? {
                return Ok(cached);
            }
            let result = ut.rest_inner(legs)?;
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

    fn repo_hold(&mut self, typ: Noun, inner: Noun, hoon_noun: Noun) -> Result<Noun> {
        let legs = [(inner, hoon_noun)];
        let leg_id = self.hold_repo_fan_leg_id_for_hold_type(typ, inner, hoon_noun)?;
        let legs_noun = self.rest_legs_noun(&legs);
        self.with_rest_leg_id(leg_id, |ut| {
            if let Some(cached) = ut.rest_boundary_lookup(typ, legs_noun)? {
                return Ok(cached);
            }
            let result = ut.rest_inner(&legs)?;
            ut.rest_boundary_store(typ, legs_noun, result)?;
            Ok(result)
        })
    }

    pub(super) fn ty_hold_cached(&mut self, inner: Noun, hoon: Noun) -> Result<Noun> {
        let raw_key = (unsafe { inner.as_raw() }, unsafe { hoon.as_raw() });
        if let Some(cached) = self.hold_memo.hold_type_raw.get(&raw_key) {
            return Ok(cached);
        }

        let space = self.slab.noun_space();
        let key = (self.noun_mug_cached(inner), self.noun_mug_cached(hoon));
        if let Some(entries) = self.hold_memo.hold_type.get(&key) {
            let inner_raw = unsafe { inner.as_raw() };
            let hoon_raw = unsafe { hoon.as_raw() };
            for entry in entries.iter().rev() {
                let inner_match = unsafe { entry.inner.raw_equals(&inner) }
                    || unsafe { entry.inner.as_raw() } == inner_raw
                    || noun_eq(entry.inner, inner, &space)?;
                if !inner_match {
                    continue;
                }
                let hoon_match = unsafe { entry.hoon.raw_equals(&hoon) }
                    || unsafe { entry.hoon.as_raw() } == hoon_raw
                    || noun_eq(entry.hoon, hoon, &space)?;
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
    pub(super) fn repo(&mut self, typ: Noun) -> Result<Noun> {
        let space = self.slab.noun_space();
        let tag = type_tag_kind(typ, &space)?;
        match tag {
            TypeTagKind::Face => {
                let inner = type_face_inner(typ, &space)?;
                Ok(inner)
            }
            TypeTagKind::Hint => {
                let inner = type_hint_inner(typ, &space)?;
                Ok(inner)
            }
            TypeTagKind::Core => {
                let (payload, _coil) = type_core_parts(typ, &space)?;
                let noun = ty_noun(self.slab);
                cell_type(self.slab, noun, payload)
            }
            TypeTagKind::Hold => {
                let (inner, hoon_noun) = type_hold_parts(typ, &space)?;
                self.repo_hold(typ, inner, hoon_noun)
            }
            TypeTagKind::Noun => {
                let atom = ty_atom(self.slab, "$", None);
                let noun = ty_noun(self.slab);
                let cell = ty_cell(self.slab, noun, noun);
                self.fork_from_options(vec![atom, cell])
            }
            _ => Err(CompilerError::Noun("repo-fltt".to_string())),
        }
    }
}
