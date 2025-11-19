pub mod block_explorer;
mod cache;
pub mod client;
pub mod driver;
pub mod metrics;
pub mod server;

#[cfg(test)]
pub(crate) mod fixtures {
    use nockchain_math::belt::Belt;
    use nockchain_types::tx_engine::v1;

    pub fn make_balance_update(count: usize) -> (v1::BalanceUpdate, Vec<v1::Name>) {
        let mut notes = Vec::with_capacity(count);
        let mut names = Vec::with_capacity(count);

        for idx in (0..count).rev() {
            let (name, note) = make_named_note(idx as u64);
            names.push(name.clone());
            notes.push((name, v1::Note::V1(note)));
        }

        let update = v1::BalanceUpdate {
            height: v1::BlockHeight(Belt((count as u64) + 1)),
            block_id: make_hash(99),
            notes: v1::Balance(notes),
        };

        (update, names)
    }

    pub fn make_named_note(seed: u64) -> (v1::Name, v1::NoteV1) {
        let name = v1::Name::new(make_hash(seed * 2 + 1), make_hash(seed * 2 + 2));

        let data: Vec<v1::NoteDataEntry> = vec![];
        let note = v1::NoteV1 {
            version: v1::Version::V1,
            origin_page: v1::BlockHeight(Belt(seed)),
            name: name.clone(),
            note_data: v1::NoteData(data),
            assets: v1::Nicks(seed as usize),
        };

        (name.clone(), note)
    }

    pub fn make_hash(seed: u64) -> v1::Hash {
        v1::Hash([Belt(seed + 1), Belt(seed + 2), Belt(seed + 3), Belt(seed + 4), Belt(seed + 5)])
    }
}
