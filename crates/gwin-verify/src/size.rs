//! `WIDTHxHEIGHT` on the command line.

use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

impl FromStr for Size {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (w, h) = s
            .split_once('x')
            .ok_or_else(|| format!("{s:?} is not WIDTHxHEIGHT"))?;
        let parse = |v: &str| {
            v.parse::<u32>()
                .map_err(|e| format!("{s:?}: {e}"))
                .and_then(|n| {
                    if n == 0 || n % 2 == 1 {
                        Err(format!("{s:?}: sides must be even and non-zero"))
                    } else {
                        Ok(n)
                    }
                })
        };
        Ok(Self {
            width: parse(w)?,
            height: parse(h)?,
        })
    }
}

impl fmt::Display for Size {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}x{}", self.width, self.height)
    }
}

#[cfg(test)]
mod tests {
    use super::Size;

    #[test]
    fn parses_and_prints() {
        let s: Size = "3840x2400".parse().expect("valid");
        assert_eq!((s.width, s.height), (3840, 2400));
        assert_eq!(s.to_string(), "3840x2400");
    }

    #[test]
    fn rejects_odd_zero_and_junk() {
        for bad in ["3840", "0x2", "3x4", "ax4", "4x"] {
            assert!(bad.parse::<Size>().is_err(), "{bad}");
        }
    }
}
