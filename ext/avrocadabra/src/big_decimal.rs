use apache_avro::BigDecimal;

pub fn encode(value: &BigDecimal) -> Result<Vec<u8>, String> {
    let (coefficient, scale) = value.as_bigint_and_exponent();
    let coefficient = coefficient.to_signed_bytes_be();
    let length = i64::try_from(coefficient.len()).map_err(|_| "big-decimal is too large")?;
    let capacity = coefficient
        .len()
        .checked_add(20)
        .ok_or("big-decimal is too large")?;
    let mut output = Vec::new();
    output
        .try_reserve_exact(capacity)
        .map_err(|e| e.to_string())?;
    write_long(length, &mut output);
    output.extend_from_slice(&coefficient);
    write_long(scale, &mut output);
    Ok(output)
}

pub fn decode(bytes: &[u8]) -> Result<BigDecimal, String> {
    let (coefficient, scale) = parts(bytes)?;
    Ok(BigDecimal::new(
        apache_avro::Decimal::from(coefficient).into(),
        scale,
    ))
}

pub fn validate(bytes: &[u8]) -> Result<(), String> {
    parts(bytes).map(|_| ())
}

fn parts(mut bytes: &[u8]) -> Result<(&[u8], i64), String> {
    let length = usize::try_from(read_long(&mut bytes)?)
        .map_err(|_| "big-decimal has a negative coefficient length")?;
    if length == 0 {
        return Err("big-decimal requires at least one signed coefficient byte".into());
    }
    let coefficient = bytes
        .get(..length)
        .ok_or("truncated big-decimal coefficient")?;
    bytes = &bytes[length..];
    let scale = read_long(&mut bytes)?;
    if !bytes.is_empty() {
        return Err("trailing bytes inside big-decimal".into());
    }
    Ok((coefficient, scale))
}

fn read_long(bytes: &mut &[u8]) -> Result<i64, String> {
    let mut value = 0_u64;
    for shift in (0..70).step_by(7) {
        let (&byte, rest) = bytes.split_first().ok_or("truncated big-decimal varint")?;
        *bytes = rest;
        if shift == 63 && byte > 1 {
            return Err("big-decimal varint overflows 64 bits".into());
        }
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(((value >> 1) as i64) ^ -((value & 1) as i64));
        }
    }
    Err("unterminated big-decimal varint".into())
}

pub(crate) fn write_long(value: i64, output: &mut Vec<u8>) {
    let mut value = ((value as u64) << 1) ^ ((value >> 63) as u64);
    while value > 0x7f {
        output.push((value as u8) | 0x80);
        value >>= 7;
    }
    output.push(value as u8);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_spans_the_rust_wire_domain_without_expansion() {
        for scale in [i64::MIN, -1, 0, 1, i64::MAX] {
            let value = BigDecimal::new((-129).into(), scale);
            let bytes = encode(&value).unwrap();
            assert!(bytes.len() <= 13);
            assert_eq!(
                decode(&bytes).unwrap().as_bigint_and_exponent(),
                ((-129).into(), scale)
            );
        }
    }

    #[test]
    fn rejects_malformed_internal_frames() {
        for bytes in [
            vec![],
            vec![0, 0],
            vec![1, 0],
            vec![4, 1],
            vec![2, 1],
            vec![2, 1, 0, 0],
            vec![2, 1, 0x80],
            vec![0xff; 10],
        ] {
            assert!(validate(&bytes).is_err(), "{bytes:?}");
        }
    }
}
