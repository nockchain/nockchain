use ibig::ops::DivRem;
use ibig::UBig;
use nockvm::interpreter::Context;
use nockvm::jets::util::{slot, BAIL_FAIL};
use nockvm::jets::JetErr;
use nockvm::noun::{Atom, Noun, NounSpace, D, T};

const BASE58_ALPHABET: &[u8; 58] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

pub fn en_base58_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let sam = slot(subject, 6, &space)?;
    let dat = sam.in_space(&space).as_atom()?;

    if dat.as_u64().map(|n| n == 0).unwrap_or(false) {
        return Ok(D(0));
    }

    let mut num = dat.as_ubig(&mut context.stack);
    let base = UBig::from(58u32);

    let mut chars = Vec::new();
    while num > UBig::from(0u32) {
        let (quotient, remainder) = num.div_rem(&base);
        let idx = usize::try_from(&remainder).map_err(|_| BAIL_FAIL)?;
        chars.push(BASE58_ALPHABET[idx]);
        num = quotient;
    }

    let mut result = D(0);
    for c in chars.iter() {
        result = T(&mut context.stack, &[D(*c as u64), result]);
    }

    Ok(result)
}

pub fn de_base58_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    let sam = slot(subject, 6, &space)?;
    let chars = tape_to_vec(sam, &space)?;

    if chars.is_empty() {
        return Ok(D(0));
    }

    let mut result = UBig::from(0u32);
    let base = UBig::from(58u32);

    for c in chars.iter() {
        let digit_value = base58_char_value(*c)?;
        result = result * &base + UBig::from(digit_value);
    }

    Ok(Atom::from_ubig(&mut context.stack, &result).as_noun())
}

fn tape_to_vec(noun: Noun, space: &NounSpace) -> Result<Vec<u8>, JetErr> {
    let mut result = Vec::new();
    let mut current = noun;

    while let Ok(cell) = current.in_space(space).as_cell() {
        let c = cell.head().as_atom()?.as_u64()? as u8;
        result.push(c);
        current = cell.tail().noun();
    }

    Ok(result)
}

fn base58_char_value(c: u8) -> Result<u32, JetErr> {
    match c {
        b'1'..=b'9' => Ok((c - b'1') as u32),
        b'A'..=b'H' => Ok((c - b'A' + 9) as u32),
        b'J'..=b'N' => Ok((c - b'J' + 17) as u32),
        b'P'..=b'Z' => Ok((c - b'P' + 22) as u32),
        b'a'..=b'k' => Ok((c - b'a' + 33) as u32),
        b'm'..=b'z' => Ok((c - b'm' + 44) as u32),
        _ => Err(BAIL_FAIL),
    }
}

#[cfg(test)]
mod tests {
    use nockvm::jets::util::test::{assert_jet, init_context};
    use nockvm::noun::{D, T};

    use super::*;

    #[test]
    fn test_en_base58_zero() {
        let mut context = init_context();
        assert_jet(&mut context, en_base58_jet, D(0), D(0));
    }

    #[test]
    fn test_en_base58_small() {
        let mut context = init_context();
        let inner = T(&mut context.stack, &[D(49), D(0)]);
        let expected = T(&mut context.stack, &[D(50), inner]);
        assert_jet(&mut context, en_base58_jet, D(58), expected);
    }

    #[test]
    fn test_de_base58_empty() {
        let mut context = init_context();
        assert_jet(&mut context, de_base58_jet, D(0), D(0));
    }

    #[test]
    fn test_de_base58_one() {
        let mut context = init_context();
        let tape = T(&mut context.stack, &[D(49), D(0)]);
        assert_jet(&mut context, de_base58_jet, tape, D(0));
    }

    #[test]
    fn test_de_base58_two() {
        let mut context = init_context();
        let tape = T(&mut context.stack, &[D(50), D(0)]);
        assert_jet(&mut context, de_base58_jet, tape, D(1));
    }

    #[test]
    fn test_base58_roundtrip() {
        let mut context = init_context();
        let original = D(123456789);
        let encode_subject = T(&mut context.stack, &[D(0), original, D(0)]);
        let encoded = en_base58_jet(&mut context, encode_subject).expect("encode should succeed");
        let decode_subject = T(&mut context.stack, &[D(0), encoded, D(0)]);
        let decoded = de_base58_jet(&mut context, decode_subject).expect("decode should succeed");
        let space = context.stack.noun_space();

        assert_eq!(
            decoded
                .in_space(&space)
                .as_atom()
                .unwrap()
                .as_u64()
                .unwrap(),
            original
                .in_space(&space)
                .as_atom()
                .unwrap()
                .as_u64()
                .unwrap()
        );
    }
}
