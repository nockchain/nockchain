use nockvm::ext::AtomExt;
use nockvm::interpreter::Context;
use nockvm::jets::util::{slot, BAIL_FAIL};
use nockvm::jets::JetErr;
use nockvm::noun::{Atom, Noun, D};
use noun_serde::{NounDecode, NounEncode};

use crate::form::belt::Belt;
use crate::form::crypto::cheetah::{CheetahPoint, F6lt, A_ID};

pub fn ser_a_pt_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let sam = slot(subject, 6, &space)?;
    let pt = CheetahPoint::from_noun(&sam, &space).map_err(|_| BAIL_FAIL)?;

    if pt == A_ID || pt.inf || !pt.in_curve() {
        return Err(BAIL_FAIL);
    }

    let mut bytes = [0u8; 104];

    for (i, belt) in pt.x.0.iter().enumerate() {
        let offset = i * 8;
        bytes[offset..offset + 8].copy_from_slice(&belt.0.to_le_bytes());
    }

    for (i, belt) in pt.y.0.iter().enumerate() {
        let offset = (i + 6) * 8;
        bytes[offset..offset + 8].copy_from_slice(&belt.0.to_le_bytes());
    }

    bytes[96] = 1;

    let actual_len = bytes
        .iter()
        .rposition(|&b| b != 0)
        .map(|p| p + 1)
        .unwrap_or(0);

    if actual_len == 0 {
        return Ok(D(0));
    }

    Ok(Atom::from_bytes(&mut context.stack, &bytes[..actual_len]).as_noun())
}

pub fn de_a_pt_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let sam = slot(subject, 6, &space)?;
    let atom = sam.in_space(&space).as_atom()?.atom();

    if atom.in_space(&space).size() < 12 {
        return Err(BAIL_FAIL);
    }

    let mut words = [0u64; 13];
    let atom_handle = atom.in_space(&space);
    let atom_bytes = atom_handle.as_ne_bytes();

    for (i, word) in words.iter_mut().enumerate() {
        let start = i * 8;
        if start >= atom_bytes.len() {
            break;
        }
        let end = std::cmp::min(start + 8, atom_bytes.len());
        let mut buf = [0u8; 8];
        buf[..end - start].copy_from_slice(&atom_bytes[start..end]);
        *word = u64::from_le_bytes(buf);
    }

    let x = F6lt([
        Belt(words[0]),
        Belt(words[1]),
        Belt(words[2]),
        Belt(words[3]),
        Belt(words[4]),
        Belt(words[5]),
    ]);
    let y = F6lt([
        Belt(words[6]),
        Belt(words[7]),
        Belt(words[8]),
        Belt(words[9]),
        Belt(words[10]),
        Belt(words[11]),
    ]);

    let pt = CheetahPoint { x, y, inf: false };
    if !pt.in_curve() {
        return Err(BAIL_FAIL);
    }

    Ok(pt.to_noun(&mut context.stack))
}

#[cfg(test)]
mod tests {
    use nockvm::jets::util::test::init_context;

    use super::*;
    use crate::form::crypto::cheetah::A_GEN;

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_ser_a_pt_roundtrip() {
        let mut context = init_context();
        let pt = A_GEN;
        let pt_noun = pt.to_noun(&mut context.stack);

        let subject = nockvm::noun::T(
            &mut context.stack,
            &[nockvm::noun::D(0), pt_noun, nockvm::noun::D(0)],
        );
        let serialized = ser_a_pt_jet(&mut context, subject).expect("serialize should succeed");

        let subject2 = nockvm::noun::T(
            &mut context.stack,
            &[nockvm::noun::D(0), serialized, nockvm::noun::D(0)],
        );
        let deserialized = de_a_pt_jet(&mut context, subject2).expect("deserialize should succeed");

        let space = context.stack.noun_space();
        let pt_back = CheetahPoint::from_noun(&deserialized, &space).expect("should decode");
        assert_eq!(pt, pt_back);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_ser_a_pt_generator() {
        let mut context = init_context();
        let pt_noun = A_GEN.to_noun(&mut context.stack);

        let subject = nockvm::noun::T(
            &mut context.stack,
            &[nockvm::noun::D(0), pt_noun, nockvm::noun::D(0)],
        );
        let result = ser_a_pt_jet(&mut context, subject);

        assert!(result.is_ok(), "should serialize generator point");
        let space = context.stack.noun_space();
        let atom = result
            .unwrap()
            .in_space(&space)
            .as_atom()
            .expect("should be atom");
        assert!(atom.size() > 0, "should have non-zero size");
    }

    #[test]
    fn test_ser_a_pt_identity_fails() {
        let mut context = init_context();
        let pt_noun = A_ID.to_noun(&mut context.stack);

        let subject = nockvm::noun::T(
            &mut context.stack,
            &[nockvm::noun::D(0), pt_noun, nockvm::noun::D(0)],
        );
        assert!(ser_a_pt_jet(&mut context, subject).is_err());
    }

    #[test]
    fn test_ser_a_pt_off_curve_fails() {
        let mut context = init_context();
        let mut bad = A_GEN;
        bad.x.0[0] = Belt(bad.x.0[0].0.wrapping_add(1));
        assert!(!bad.in_curve(), "test point must be off-curve");

        let bad_noun = bad.to_noun(&mut context.stack);
        let subject = nockvm::noun::T(
            &mut context.stack,
            &[nockvm::noun::D(0), bad_noun, nockvm::noun::D(0)],
        );
        assert!(ser_a_pt_jet(&mut context, subject).is_err());
    }

    #[test]
    fn test_de_a_pt_off_curve_fails() {
        let mut context = init_context();
        let mut bad = A_GEN;
        bad.x.0[0] = Belt(bad.x.0[0].0.wrapping_add(1));
        assert!(!bad.in_curve(), "test point must be off-curve");

        let mut bytes = [0u8; 104];
        for (i, belt) in bad.x.0.iter().enumerate() {
            let offset = i * 8;
            bytes[offset..offset + 8].copy_from_slice(&belt.0.to_le_bytes());
        }
        for (i, belt) in bad.y.0.iter().enumerate() {
            let offset = (i + 6) * 8;
            bytes[offset..offset + 8].copy_from_slice(&belt.0.to_le_bytes());
        }
        bytes[96] = 1;

        let actual_len = bytes
            .iter()
            .rposition(|&b| b != 0)
            .map(|p| p + 1)
            .unwrap_or(0);
        let atom = Atom::from_bytes(&mut context.stack, &bytes[..actual_len]);

        let subject = nockvm::noun::T(
            &mut context.stack,
            &[nockvm::noun::D(0), atom.as_noun(), nockvm::noun::D(0)],
        );
        assert!(de_a_pt_jet(&mut context, subject).is_err());
    }

    #[test]
    fn test_de_a_pt_short_input_fails() {
        let mut context = init_context();
        let mut bytes = [0u8; 88];
        bytes[0] = 1;
        let atom = Atom::from_bytes(&mut context.stack, &bytes);

        let subject = nockvm::noun::T(
            &mut context.stack,
            &[nockvm::noun::D(0), atom.as_noun(), nockvm::noun::D(0)],
        );
        assert!(de_a_pt_jet(&mut context, subject).is_err());
    }
}
