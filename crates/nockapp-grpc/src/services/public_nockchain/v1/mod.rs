mod cache;
pub mod client;
pub mod driver;
pub mod metrics;
pub mod server;

#[cfg(test)]
pub(crate) mod fixtures {
    use nockchain_math::belt::Belt;
    use nockchain_math::crypto::cheetah::A_GEN;
    use nockchain_types::tx_engine::v0;

    pub fn make_balance_update(count: usize) -> (v0::BalanceUpdate, Vec<v0::Name>) {
        let mut notes = Vec::with_capacity(count);
        let mut names = Vec::with_capacity(count);

        for idx in (0..count).rev() {
            let (name, note) = make_named_note(idx as u64);
            names.push(name.clone());
            notes.push((name, note));
        }

        let update = v0::BalanceUpdate {
            height: v0::BlockHeight(Belt((count as u64) + 1)),
            block_id: make_hash(99),
            notes: v0::Balance(notes),
        };

        (update, names)
    }

    pub fn make_named_note(seed: u64) -> (v0::Name, v0::NoteV0) {
        let name = v0::Name::new(make_hash(seed * 2 + 1), make_hash(seed * 2 + 2));

        let head = v0::NoteHead {
            version: v0::Version::V0,
            origin_page: v0::BlockHeight(Belt(seed)),
            timelock: v0::Timelock(None),
        };

        let tail = v0::NoteTail {
            name: name.clone(),
            lock: v0::Lock {
                keys_required: 1,
                pubkeys: vec![v0::SchnorrPubkey(A_GEN)],
            },
            source: v0::Source {
                hash: make_hash(seed * 3 + 7),
                is_coinbase: false,
            },
            assets: v0::Nicks(seed as usize),
        };

        (name.clone(), v0::NoteV0 { head, tail })
    }

    pub fn make_hash(seed: u64) -> v0::Hash {
        v0::Hash([Belt(seed + 1), Belt(seed + 2), Belt(seed + 3), Belt(seed + 4), Belt(seed + 5)])
    }
}
