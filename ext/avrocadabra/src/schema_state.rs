use magnus::{
    Error, RArray, RHash, RString, Value, encoding::EncodingCapable, prelude::*, r_hash::ForEach,
    rb_sys::AsRawValue,
};

fn identical(left: Value, right: Value) -> bool {
    left.as_raw() == right.as_raw()
}

pub fn unchanged(ruby: &magnus::Ruby, containers: RArray) -> Result<bool, Error> {
    contents_unchanged(containers).map_err(|error| crate::materialize(ruby, error))
}

fn contents_unchanged(containers: RArray) -> Result<bool, Error> {
    for index in (0..containers.len() as isize).step_by(2) {
        let object: Value = containers.entry(index)?;
        let previous: Value = containers.entry(index + 1)?;
        if let Some(string) = RString::from_value(object) {
            let previous = RString::try_convert(previous)?;
            if string.enc_get() != previous.enc_get()
                || unsafe { string.as_slice() != previous.as_slice() }
            {
                return Ok(false);
            }
        } else if let Some(array) = RArray::from_value(object) {
            let previous = RArray::try_convert(previous)?;
            if array.len() != previous.len()
                || !unsafe { array.as_slice().iter().zip(previous.as_slice()) }
                    .all(|(&left, &right)| identical(left, right))
            {
                return Ok(false);
            }
        } else if let Some(hash) = RHash::from_value(object) {
            let previous = RArray::try_convert(previous)?;
            if hash.len() * 2 != previous.len() {
                return Ok(false);
            }
            let mut index = 0;
            let mut matches = true;
            hash.foreach(|key: Value, value: Value| {
                matches = identical(key, previous.entry(index)?)
                    && identical(value, previous.entry(index + 1)?);
                index += 2;
                Ok(if matches {
                    ForEach::Continue
                } else {
                    ForEach::Stop
                })
            })?;
            if !matches {
                return Ok(false);
            }
        }
    }
    Ok(true)
}
