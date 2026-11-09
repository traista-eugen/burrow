//! Entity ids: a type prefix, an underscore, then a UUIDv7 in Crockford base32.
//!
//! For example `con_01K9QZ3V7W2XG8M4T6A1B5C9DE`. The base32 form is 26 characters, the same as a
//! ULID: shorter than hex, free of ambiguous letters, and it sorts in creation order like the
//! UUIDv7 inside it. Clients treat ids as opaque strings (protocol.md, "Naming and value
//! formats").

use uuid::Uuid;

/// Crockford's base32 alphabet: digits and letters without I, L, O and U.
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// A new id with the given prefix, for example `new_id("con")`.
pub fn new_id(prefix: &str) -> String {
    format!("{prefix}_{}", base32(Uuid::now_v7().as_u128()))
}

/// 128 bits as 26 base32 characters, most significant first. The first character holds the top
/// 3 bits, the other 25 hold 5 bits each.
fn base32(value: u128) -> String {
    (0..26)
        .rev()
        .map(|digit| char::from(ALPHABET[((value >> (digit * 5)) & 0x1f) as usize]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_have_a_prefix_and_26_base32_characters() {
        let id = new_id("con");
        let (prefix, body) = id.split_once('_').unwrap();
        assert_eq!(prefix, "con");
        assert_eq!(body.len(), 26);
        assert!(body.bytes().all(|byte| ALPHABET.contains(&byte)), "{id}");
    }

    #[test]
    fn base32_keeps_numeric_order() {
        assert_eq!(base32(0), "0".repeat(26));
        assert_eq!(base32(u128::MAX), format!("7{}", "Z".repeat(25)));
        assert_eq!(base32(31), format!("{}Z", "0".repeat(25)));
        assert_eq!(base32(32), format!("{}10", "0".repeat(24)));
        let values = [0, 1, 31, 32, 1 << 64, (1 << 125) - 1, 1 << 125, u128::MAX];
        for pair in values.windows(2) {
            assert!(base32(pair[0]) < base32(pair[1]));
        }
    }

    #[test]
    fn ids_sort_in_creation_order() {
        let ids: Vec<String> = (0..100).map(|_| new_id("con")).collect();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(ids, sorted);
    }
}
