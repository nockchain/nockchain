use nockvm::noun::{AtomHandle, NounSpace};

use super::*;

fn axis_from_u64(axis: u64) -> BigUint {
    BigUint::from(axis)
}

fn prepend_vein(lon: &[Option<BigUint>], step: Option<BigUint>) -> Vec<Option<BigUint>> {
    let mut vein = Vec::with_capacity(lon.len() + 1);
    vein.push(step);
    vein.extend_from_slice(lon);
    vein
}

fn atom_handle_to_string(atom: AtomHandle<'_>) -> Result<String> {
    if let Ok(value) = atom.as_u64() {
        if value == 0 {
            return Ok("$".to_string());
        }
    }
    atom.into_string().map_err(|err| {
        let location = std::panic::Location::caller();
        let bytes = atom.as_ne_bytes();
        let preview_len = bytes.len().min(16);
        let preview = bytes[..preview_len]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<Vec<_>>()
            .join(" ");
        CompilerError::Decode(format!(
            "atom decode failed at {location}: {err}; bytes={preview}"
        ))
    })
}

impl<'a> Ut<'a> {
    // HOON138:arm=ut:find lines=9472-9484 map=direct status=partial reviewed=2026-03-06
    // HOON138_NOTE:native primary implementation for canonical `++find`; full parity review is still in progress
    #[track_caller]
    pub(super) fn find(&mut self, sut: Noun, way: Way, wing: &WingType) -> Result<Port> {
        let result = match self.fond(sut, way, wing)? {
            Pony::Palo(palo) => Ok(Port::Palo(palo)),
            Pony::Synthetic { typ, formula } => Ok(Port::Synthetic { typ, formula }),
            Pony::Void | Pony::Unmatched(_) => Err(CompilerError::UnsupportedExpr(format!(
                "native mint: find failed for wing {wing:?}"
            ))),
        };
        result
    }

    pub(super) fn fond_hold_inner(&mut self, sut: Noun) -> Result<Noun> {
        self.repo(sut)
    }

    // HOON138:arm=ut:fend lines=9486-9492 map=direct status=partial reviewed=2026-03-06
    // HOON138_NOTE:native primary implementation for canonical `++fend`; full parity review is still in progress
    pub(super) fn fend(&mut self, sut: Noun, way: Way, wing: &WingType) -> Result<(Noun, u64)> {
        let port = self.find(sut, way, wing)?;
        match port {
            Port::Palo(palo) => match palo.opal {
                Opal::Leg(typ) => Ok((typ, tend(&palo.vein)?)),
                Opal::Arm { .. } => Err(CompilerError::Noun("fend-fragment".to_string())),
            },
            Port::Synthetic { .. } => Err(CompilerError::Noun("fend-fragment".to_string())),
        }
    }

    // HOON138:arm=ut:fund lines=9494-9501 map=direct status=partial reviewed=2026-03-06
    // HOON138_NOTE:native primary implementation for canonical `++fund`; full parity review is still in progress
    pub(super) fn fund(&mut self, sut: Noun, way: Way, gen: &Hoon) -> Result<Port> {
        if let Some(wing) = reek(gen.clone()) {
            return self.find(sut, way, &wing);
        }
        let goal = ty_noun(self.slab);
        let (typ, formula) = self.mint(sut, goal, gen)?;
        Ok(Port::Synthetic { typ, formula })
    }

    // HOON138:arm=ut:fond lines=9311-9470 map=direct status=partial reviewed=2026-03-06
    // HOON138_NOTE:native primary implementation for canonical `++fond`; full parity review is still in progress
    pub(super) fn fond(&mut self, sut: Noun, way: Way, wing: &[Limb]) -> Result<Pony> {
        if wing.is_empty() {
            let out = Pony::Palo(Palo {
                vein: Vec::new(),
                opal: Opal::Leg(sut),
            });
            return Ok(out);
        }

        let (head, tail) = wing
            .split_first()
            .ok_or_else(|| CompilerError::UnsupportedExpr("native mint: empty wing".to_string()))?;
        let mor = self.fond(sut, way, tail)?;

        let out = match mor {
            Pony::Void => Ok(Pony::Void),
            Pony::Unmatched(skip) => Ok(Pony::Unmatched(skip)),
            Pony::Synthetic { typ, formula } => {
                let goal = ty_noun(self.slab);
                let (new_ty, new_formula) =
                    self.mint(typ, goal, &Hoon::Wing(vec![head.clone()]))?;
                let combined = comb(self.slab, formula, new_formula)?;
                Ok(Pony::Synthetic {
                    typ: new_ty,
                    formula: combined,
                })
            }
            Pony::Palo(palo) => {
                let current_sut = match &palo.opal {
                    Opal::Leg(typ) => *typ,
                    Opal::Arm { arms, .. } => {
                        let mut types = Vec::with_capacity(arms.len());
                        for (typ, _) in arms {
                            types.push(*typ);
                        }
                        self.fork_from_options(types)?
                    }
                };
                let lon = palo.vein;
                match head {
                    Limb::Axis(step) => {
                        let typ = self.peek(current_sut, way, *step)?;
                        let vein = prepend_vein(&lon, Some(axis_from_u64(*step)));
                        Ok(Pony::Palo(Palo {
                            vein,
                            opal: Opal::Leg(typ),
                        }))
                    }
                    Limb::Term(name) => {
                        self.fond_name(current_sut, way, 0, Some(name.as_str()), lon)
                    }
                    Limb::Parent(skip, maybe_name) => {
                        self.fond_name(current_sut, way, *skip, maybe_name.as_deref(), lon)
                    }
                }
            }
        };
        out
    }

    // HOON138:arm=ut:fond lines=9311-9470 map=aux status=partial reviewed=2026-03-06
    // HOON138_NOTE:recursive inner branch walker for canonical `++fond`
    fn fond_name(
        &mut self,
        sut: Noun,
        way: Way,
        skip: u64,
        name: Option<&str>,
        lon: Vec<Option<BigUint>>,
    ) -> Result<Pony> {
        fn compose_axis_formula(ut: &mut Ut<'_>, axe: BigUint, formula: Noun) -> Result<Noun> {
            let axis_formula = slot_formula_axis_big(ut.slab, axe);
            comb(ut.slab, axis_formula, formula)
        }

        fn here(sut: Noun, axe: &BigUint, skip: u64, lon: &[Option<BigUint>]) -> Pony {
            if skip > 0 {
                return Pony::Unmatched(skip.saturating_sub(1));
            }
            let mut vein = Vec::with_capacity(lon.len() + 2);
            vein.push(None);
            vein.push(Some(axe.clone()));
            vein.extend_from_slice(lon);
            Pony::Palo(Palo {
                vein,
                opal: Opal::Leg(sut),
            })
        }

        fn lose(skip: u64) -> Pony {
            Pony::Unmatched(skip)
        }

        fn stop(
            sut: Noun,
            axe: &BigUint,
            skip: u64,
            name: Option<&str>,
            lon: &[Option<BigUint>],
        ) -> Pony {
            if name.is_none() {
                here(sut, axe, skip, lon)
            } else {
                lose(skip)
            }
        }

        fn is_term_face(space: &NounSpace, sut: Noun) -> Result<Option<String>> {
            let cell = sut
                .in_space(space)
                .as_cell()
                .map_err(|err| CompilerError::Decode(format!("face not cell in find: {err}")))?;
            let tail = cell.tail().as_cell().map_err(|err| {
                CompilerError::Decode(format!("face missing tail in find: {err}"))
            })?;
            let name_atom = match tail.head().as_atom() {
                Ok(atom) => atom,
                Err(_) => return Ok(None),
            };
            Ok(Some(atom_handle_to_string(name_atom).map_err(|err| {
                CompilerError::Decode(format!("face name in find: {err}"))
            })?))
        }

        fn face_tool_tune_parts(space: &NounSpace, tool: Noun) -> Result<Option<(Noun, Noun)>> {
            let tool_handle = tool.in_space(space);
            if tool_handle.as_atom().is_ok() {
                return Ok(None);
            }
            let tool_cell = tool_handle.as_cell().map_err(|err| {
                CompilerError::Decode(format!("face tool not cell in find: {err}"))
            })?;
            let head = tool_cell.head();
            let tail = tool_cell.tail();

            if let Ok(tag_atom) = head.as_atom() {
                if let Ok(tag) = atom_handle_to_string(tag_atom) {
                    if tag == "tune" {
                        let tune_cell = tail.as_cell().map_err(|err| {
                            CompilerError::Decode(format!(
                                "face tune payload not cell in find: {err}"
                            ))
                        })?;
                        return Ok(Some((tune_cell.head().noun(), tune_cell.tail().noun())));
                    }
                }
            }

            Ok(Some((head.noun(), tail.noun())))
        }

        fn unit_hoon_value(space: &NounSpace, unit: Noun) -> Result<Option<Noun>> {
            if noun_is_zero(unit) {
                return Ok(None);
            }
            let unit_cell = unit.in_space(space).as_cell().map_err(|err| {
                CompilerError::Decode(format!("face tune unit not cell in find: {err}"))
            })?;
            let tag_atom = unit_cell.head().as_atom().map_err(|err| {
                CompilerError::Decode(format!("face tune unit tag not atom in find: {err}"))
            })?;
            let tag = tag_atom.as_u64().map_err(|err| {
                CompilerError::Decode(format!("face tune unit tag decode in find: {err}"))
            })?;
            if tag != 0 {
                return Err(CompilerError::Decode(format!(
                    "face tune unit tag mismatch in find: expected 0, got {tag}"
                )));
            }
            Ok(Some(unit_cell.tail().noun()))
        }

        #[derive(Default)]
        struct SeenState {
            hold_path: Vec<Noun>,
        }

        fn go(
            ut: &mut Ut<'_>,
            sut: Noun,
            way: Way,
            axe: BigUint,
            skip: u64,
            name: Option<&str>,
            lon: &[Option<BigUint>],
            seen: &mut SeenState,
        ) -> Result<Pony> {
            {
                let space = ut.slab.noun_space();
                let tag = type_tag(sut, &space)?;
                match tag.as_str() {
                    "void" => Ok(Pony::Void),
                    "noun" => Ok(stop(sut, &axe, skip, name, lon)),
                    "atom" => Ok(stop(sut, &axe, skip, name, lon)),
                    "cell" => {
                        if name.is_none() {
                            return Ok(here(sut, &axe, skip, lon));
                        }
                        let (head, tail) = type_cell_parts(sut, &space)?;
                        let head_res = go(
                            ut,
                            head,
                            way,
                            peg_axis_big(axe.clone(), 2)?,
                            skip,
                            name,
                            lon,
                            seen,
                        )?;
                        let searched = match head_res {
                            Pony::Void => Ok(Pony::Void),
                            Pony::Palo(_) | Pony::Synthetic { .. } => Ok(head_res),
                            Pony::Unmatched(rem) => go(
                                ut,
                                tail,
                                way,
                                peg_axis_big(axe.clone(), 3)?,
                                rem,
                                name,
                                lon,
                                seen,
                            ),
                        }?;
                        Ok(searched)
                    }
                    "face" => {
                        let inner = type_face_inner(sut, &space)?;
                        let tool = type_face_tool(sut, &space)?;
                        if name.is_none() {
                            return Ok(here(inner, &axe, skip, lon));
                        }
                        let term_face_name = {
                            let space = ut.slab.noun_space();
                            is_term_face(&space, sut)?
                        };
                        if let Some(face_name) = term_face_name {
                            if Some(face_name.as_str()) == name {
                                if skip == 0 {
                                    return Ok(here(inner, &axe, skip, lon));
                                }
                                return Ok(Pony::Unmatched(skip.saturating_sub(1)));
                            }
                            return Ok(lose(skip));
                        }
                        let search_name = name.unwrap_or_default();
                        let (aliases, mut bridges) = {
                            let space = ut.slab.noun_space();
                            face_tool_tune_parts(&space, tool)?.ok_or_else(|| {
                                CompilerError::UnsupportedExpr(
                                    "native mint: strict find face non-term tool".to_string(),
                                )
                            })?
                        };
                        let cog = term_to_noun(ut.slab, search_name);
                        let mut rem = skip;
                        let alias_lookup = ut.look(cog, aliases)?;
                        if let Some((_tool_axis, unit_value)) = alias_lookup {
                            let unit_value = {
                                let space = ut.slab.noun_space();
                                unit_hoon_value(&space, unit_value)?
                            };
                            match unit_value {
                                None => {
                                    let lon_with_face = prepend_vein(lon, None);
                                    return go(
                                        ut,
                                        inner,
                                        way,
                                        axe.clone(),
                                        rem.saturating_add(1),
                                        name,
                                        &lon_with_face,
                                        seen,
                                    );
                                }
                                Some(hoon_noun) => {
                                    if rem == 0 {
                                        let hoon_ast =
                                            ut.hoon_ast_lookup(hoon_noun).ok_or_else(|| {
                                                CompilerError::Noun(
                                                    "face tune bridge ast missing".to_string(),
                                                )
                                            })?;
                                        let tor = ut.fund(sut, way, hoon_ast.as_ref())?;
                                        return match tor {
                                            Port::Palo(palo) => {
                                                let mut vein = palo.vein;
                                                vein.push(None);
                                                vein.push(Some(axe.clone()));
                                                vein.extend_from_slice(lon);
                                                Ok(Pony::Palo(Palo {
                                                    vein,
                                                    opal: palo.opal,
                                                }))
                                            }
                                            Port::Synthetic { typ, formula } => {
                                                let formula =
                                                    compose_axis_formula(ut, axe.clone(), formula)?;
                                                Ok(Pony::Synthetic { typ, formula })
                                            }
                                        };
                                    }
                                    rem = rem.saturating_sub(1);
                                }
                            }
                        }
                        loop {
                            if noun_is_zero(bridges) {
                                let lon_with_face = prepend_vein(lon, None);
                                return go(
                                    ut,
                                    inner,
                                    way,
                                    axe.clone(),
                                    rem,
                                    name,
                                    &lon_with_face,
                                    seen,
                                );
                            }
                            let space = ut.slab.noun_space();
                            let bridge_cell =
                                bridges.in_space(&space).as_cell().map_err(|err| {
                                    CompilerError::Decode(format!(
                                        "face tune bridge list not cell in find: {err}"
                                    ))
                                })?;
                            let bridge_hoon_noun = bridge_cell.head().noun();
                            bridges = bridge_cell.tail().noun();
                            let bridge_hoon_ast =
                                ut.hoon_ast_lookup(bridge_hoon_noun).ok_or_else(|| {
                                    CompilerError::Noun(
                                        "face tune bridge expression ast missing".to_string(),
                                    )
                                })?;
                            let noun_goal = ty_noun(ut.slab);
                            let (bridge_ty, bridge_formula) =
                                ut.mint(inner, noun_goal, bridge_hoon_ast.as_ref())?;
                            let mut bridge_seen = SeenState::default();
                            let fid = go(
                                ut,
                                bridge_ty,
                                way,
                                BigUint::from(1u32),
                                rem,
                                name,
                                &[],
                                &mut bridge_seen,
                            )?;
                            match fid {
                                Pony::Void => return Ok(Pony::Void),
                                Pony::Unmatched(next_rem) => {
                                    rem = next_rem;
                                    continue;
                                }
                                Pony::Palo(palo) => {
                                    let (fid_ty, fid_formula) = ut.fine(&Port::Palo(palo))?;
                                    let composed =
                                        compose_axis_formula(ut, axe.clone(), bridge_formula)?;
                                    let formula = comb(ut.slab, composed, fid_formula)?;
                                    return Ok(Pony::Synthetic {
                                        typ: fid_ty,
                                        formula,
                                    });
                                }
                                Pony::Synthetic { typ, formula } => {
                                    let composed =
                                        compose_axis_formula(ut, axe.clone(), bridge_formula)?;
                                    let formula = comb(ut.slab, composed, formula)?;
                                    return Ok(Pony::Synthetic { typ, formula });
                                }
                            }
                        }
                    }
                    "hint" => {
                        let inner = type_hint_inner(sut, &space)?;
                        go(ut, inner, way, axe, skip, name, lon, seen)
                    }
                    "hold" => {
                        for prior_hold in seen.hold_path.iter() {
                            if noun_eq(*prior_hold, sut, &space)? {
                                return Ok(Pony::Void);
                            }
                        }
                        seen.hold_path.push(sut);
                        let inner = ut.fond_hold_inner(sut)?;
                        let hold_result = go(ut, inner, way, axe.clone(), skip, name, lon, seen);
                        seen.hold_path.pop();
                        hold_result
                    }
                    "fork" => {
                        let options = type_fork_options(sut, &space)?;
                        let mut iter = options.into_iter();
                        let Some(first) = iter.next() else {
                            return Ok(Pony::Void);
                        };
                        let mut acc = go(ut, first, way, axe.clone(), skip, name, lon, seen)?;
                        for option in iter {
                            let next = go(ut, option, way, axe.clone(), skip, name, lon, seen)?;
                            acc = ut.twin(acc, next)?;
                        }
                        Ok(acc)
                    }
                    "core" => {
                        let Some(name_str) = name else {
                            return Ok(here(sut, &axe, skip, lon));
                        };
                        let (payload, coil) = type_core_parts(sut, &space)?;
                        let (garb, _context, rest) = coil_parts(coil, &space)?;
                        let poly = garb_poly(garb, &space)?;
                        let vair = garb_vair(garb, &space)?;
                        let cog = term_to_noun(ut.slab, name_str);
                        let tomes = coil_tomes(rest, &space)?;
                        let mut rem = skip;
                        if let Some((arm_axis, hoon)) = ut.loot(cog, tomes)? {
                            if rem == 0 {
                                let axis = peg_axis(2, arm_axis)?;
                                let foot = foot_from_poly(ut.slab, poly, hoon);
                                let mut vein = Vec::with_capacity(lon.len() + 1);
                                vein.push(Some(axe.clone()));
                                vein.extend_from_slice(lon);
                                return Ok(Pony::Palo(Palo {
                                    vein,
                                    opal: Opal::Arm {
                                        axis,
                                        arms: vec![(sut, foot)],
                                    },
                                }));
                            }
                            rem = rem.saturating_sub(1);
                        }
                        let (sam, con) = peel(way, vair);
                        if !sam {
                            return Ok(Pony::Unmatched(rem));
                        }
                        if con {
                            go(
                                ut,
                                payload,
                                way,
                                peg_axis_big(axe.clone(), 3)?,
                                rem,
                                name,
                                lon,
                                seen,
                            )
                        } else {
                            let peeked = ut.peek(payload, way, 2)?;
                            go(
                                ut,
                                peeked,
                                way,
                                peg_axis_big(axe.clone(), 6)?,
                                rem,
                                name,
                                lon,
                                seen,
                            )
                        }
                    }
                    _ => Ok(Pony::Void),
                }
            }
        }

        let mut seen = SeenState::default();
        go(
            self,
            sut,
            way,
            BigUint::from(1u32),
            skip,
            name,
            &lon,
            &mut seen,
        )
    }

    pub(super) fn resolve_wing_axis(&mut self, sut: Noun, wing: &WingType) -> Result<u64> {
        if wing.is_empty() {
            return Ok(1);
        }
        let (_typ, axis) = self.fend(sut, Way::Read, wing)?;
        Ok(axis)
    }

    pub(super) fn noun_seen_insert_structural(
        &mut self,
        seen: &mut HashMap<u32, Vec<Noun>>,
        noun: Noun,
    ) -> Result<bool> {
        let mug = self.noun_mug_cached(noun);
        if let Some(bucket) = seen.get(&mug) {
            for prior in bucket {
                if noun_eq(*prior, noun, &self.slab.noun_space())? {
                    return Ok(false);
                }
            }
        }
        seen.entry(mug).or_default().push(noun);
        Ok(true)
    }

    pub(super) fn twin(&mut self, left: Pony, right: Pony) -> Result<Pony> {
        match (left, right) {
            (Pony::Void, other) | (other, Pony::Void) => Ok(other),
            (Pony::Unmatched(rem), other @ Pony::Palo(_))
            | (other @ Pony::Palo(_), Pony::Unmatched(rem))
            | (Pony::Unmatched(rem), other @ Pony::Synthetic { .. })
            | (other @ Pony::Synthetic { .. }, Pony::Unmatched(rem)) => {
                let _ = (rem, other);
                Err(CompilerError::Noun("find-fork".to_string()))
            }
            (Pony::Unmatched(a), Pony::Unmatched(b)) if a == b => Ok(Pony::Unmatched(a)),
            (
                Pony::Synthetic {
                    typ: left_ty,
                    formula: left_formula,
                },
                Pony::Synthetic {
                    typ: right_ty,
                    formula: right_formula,
                },
            ) => {
                if !noun_eq(left_formula, right_formula, &self.slab.noun_space())? {
                    return Err(CompilerError::Noun("find-fork".to_string()));
                }
                let ty = self.fork_from_options(vec![left_ty, right_ty])?;
                Ok(Pony::Synthetic {
                    typ: ty,
                    formula: left_formula,
                })
            }
            (Pony::Palo(left), Pony::Palo(right)) => {
                if left.vein != right.vein {
                    return Err(CompilerError::Noun("find-fork".to_string()));
                }
                match (left.opal, right.opal) {
                    (Opal::Leg(left_ty), Opal::Leg(right_ty)) => {
                        let ty = self.fork_from_options(vec![left_ty, right_ty])?;
                        Ok(Pony::Palo(Palo {
                            vein: left.vein,
                            opal: Opal::Leg(ty),
                        }))
                    }
                    (
                        Opal::Arm {
                            axis: left_axis,
                            arms: left_arms,
                        },
                        Opal::Arm {
                            axis: right_axis,
                            arms: right_arms,
                        },
                    ) => {
                        if left_axis != right_axis {
                            return Err(CompilerError::Noun("find-fork".to_string()));
                        }
                        let mut merged: Vec<(Noun, Noun)> =
                            Vec::with_capacity(left_arms.len().saturating_add(right_arms.len()));
                        let mut seen: HashMap<(u32, u32), Vec<(Noun, Noun)>> = HashMap::new();
                        for (core, foot) in left_arms.into_iter().chain(right_arms.into_iter()) {
                            let key = (self.noun_mug_cached(core), self.noun_mug_cached(foot));
                            let duplicate = if let Some(bucket) = seen.get(&key) {
                                let mut duplicate = false;
                                for (prev_core, prev_foot) in bucket {
                                    let same_core =
                                        noun_eq(*prev_core, core, &self.slab.noun_space())?;
                                    if !same_core {
                                        continue;
                                    }
                                    let same_foot =
                                        noun_eq(*prev_foot, foot, &self.slab.noun_space())?;
                                    if same_foot {
                                        duplicate = true;
                                        break;
                                    }
                                }
                                duplicate
                            } else {
                                false
                            };
                            if !duplicate {
                                merged.push((core, foot));
                                seen.entry(key).or_default().push((core, foot));
                            }
                        }
                        Ok(Pony::Palo(Palo {
                            vein: left.vein,
                            opal: Opal::Arm {
                                axis: left_axis,
                                arms: merged,
                            },
                        }))
                    }
                    _ => Err(CompilerError::Noun("find-fork".to_string())),
                }
            }
            _ => Err(CompilerError::Noun("find-fork".to_string())),
        }
    }

    pub(super) fn look(&mut self, cog: Noun, dab: Noun) -> Result<Option<(u64, Noun)>> {
        let mut current = dab;
        let mut axe = 1u64;
        let found = loop {
            let space = self.slab.noun_space();
            let Some((node, left, right)) = map_node(current, &space)? else {
                break None;
            };

            let node_cell = node
                .in_space(&space)
                .as_cell()
                .map_err(|err| CompilerError::Decode(format!("map node not cell: {err}")))?;
            let node_key = node_cell.head().noun();
            let node_val = node_cell.tail().noun();
            let left_empty = noun_is_zero(left);
            let right_empty = noun_is_zero(right);
            if noun_eq(cog, node_key, &space)? {
                let axis = if left_empty && right_empty {
                    axe
                } else {
                    peg_axis(axe, 2)?
                };
                break Some((axis, node_val));
            }
            let go_left = gor_mug(self.slab, cog, node_key);
            match (left_empty, right_empty) {
                (true, true) => break None,
                (true, false) => {
                    if go_left {
                        break None;
                    }
                    axe = peg_axis(axe, 3)?;
                    current = right;
                }
                (false, true) => {
                    if !go_left {
                        break None;
                    }
                    axe = peg_axis(axe, 3)?;
                    current = left;
                }
                (false, false) => {
                    if go_left {
                        axe = peg_axis(axe, 6)?;
                        current = left;
                    } else {
                        axe = peg_axis(axe, 7)?;
                        current = right;
                    }
                }
            }
        };
        Ok(found)
    }

    pub(super) fn loot(&mut self, cog: Noun, dom: Noun) -> Result<Option<(u64, Noun)>> {
        let mut stack: Vec<(Noun, u64)> = vec![(dom, 1u64)];
        let mut found: Option<(u64, Noun)> = None;
        while let Some((current, axe)) = stack.pop() {
            let space = self.slab.noun_space();
            let Some((node, left, right)) = map_node(current, &space)? else {
                continue;
            };
            let node_cell = node
                .in_space(&space)
                .as_cell()
                .map_err(|err| CompilerError::Decode(format!("tome entry not cell: {err}")))?;
            let tome = node_cell.tail();
            let tome_cell = tome
                .as_cell()
                .map_err(|err| CompilerError::Decode(format!("tome value not cell: {err}")))?;
            let arms_map = tome_cell.tail().noun();
            let left_empty = noun_is_zero(left);
            let right_empty = noun_is_zero(right);
            if let Some((arm_axis, hoon)) = self.look(cog, arms_map)? {
                let axis = if left_empty && right_empty {
                    peg_axis(axe, arm_axis)?
                } else {
                    peg_axis(peg_axis(axe, 2)?, arm_axis)?
                };
                found = Some((axis, hoon));
                break;
            }
            match (left_empty, right_empty) {
                (true, true) => {}
                (true, false) => stack.push((right, peg_axis(axe, 3)?)),
                (false, true) => stack.push((left, peg_axis(axe, 3)?)),
                (false, false) => {
                    stack.push((right, peg_axis(axe, 7)?));
                    stack.push((left, peg_axis(axe, 6)?));
                }
            }
        }
        Ok(found)
    }
}
