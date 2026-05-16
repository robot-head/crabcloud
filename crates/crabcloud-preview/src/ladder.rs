//! Fixed thumbnail size ladder. Three rungs: 64, 256, 1024 px.

use crate::error::PreviewError;

pub const LADDER: &[u32] = &[64, 256, 1024];

/// Round `requested` UP to the next ladder rung. Returns
/// `Err(SizeOutOfRange)` if `requested` is above the top of the ladder.
/// `requested = 0` is treated as the smallest rung (defensive).
pub fn round_up_to_ladder(requested: u32) -> Result<u32, PreviewError> {
    if requested == 0 {
        return Ok(LADDER[0]);
    }
    if requested > *LADDER.last().expect("ladder non-empty") {
        return Err(PreviewError::SizeOutOfRange(requested));
    }
    for &rung in LADDER {
        if requested <= rung {
            return Ok(rung);
        }
    }
    unreachable!("requested <= last ladder rung was checked above")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounds_up_within_range() {
        assert_eq!(round_up_to_ladder(1).unwrap(), 64);
        assert_eq!(round_up_to_ladder(16).unwrap(), 64);
        assert_eq!(round_up_to_ladder(64).unwrap(), 64);
        assert_eq!(round_up_to_ladder(65).unwrap(), 256);
        assert_eq!(round_up_to_ladder(256).unwrap(), 256);
        assert_eq!(round_up_to_ladder(257).unwrap(), 1024);
        assert_eq!(round_up_to_ladder(1024).unwrap(), 1024);
    }

    #[test]
    fn zero_returns_smallest_rung() {
        assert_eq!(round_up_to_ladder(0).unwrap(), 64);
    }

    #[test]
    fn rejects_above_top_rung() {
        match round_up_to_ladder(1025) {
            Err(PreviewError::SizeOutOfRange(1025)) => {}
            other => panic!("expected SizeOutOfRange(1025), got {other:?}"),
        }
        match round_up_to_ladder(u32::MAX) {
            Err(PreviewError::SizeOutOfRange(_)) => {}
            other => panic!("expected SizeOutOfRange, got {other:?}"),
        }
    }

    #[test]
    fn ladder_is_strictly_increasing() {
        for w in LADDER.windows(2) {
            assert!(w[0] < w[1], "ladder must be strictly increasing");
        }
    }
}
