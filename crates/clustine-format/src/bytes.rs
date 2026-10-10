//! Reading fixed-width big-endian values from a byte slice.

use crate::FormatError;

pub(crate) struct Input<'a>(pub(crate) &'a [u8]);

impl<'a> Input<'a> {
    pub(crate) fn take(&mut self, count: usize) -> Result<&'a [u8], FormatError> {
        let (taken, rest) = self
            .0
            .split_at_checked(count)
            .ok_or(FormatError::Truncated)?;
        self.0 = rest;
        Ok(taken)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], FormatError> {
        Ok(self.take(N)?.try_into().expect("take returned N bytes"))
    }

    pub(crate) fn u8(&mut self) -> Result<u8, FormatError> {
        Ok(self.array::<1>()?[0])
    }

    pub(crate) fn u16(&mut self) -> Result<u16, FormatError> {
        Ok(u16::from_be_bytes(self.array()?))
    }

    pub(crate) fn i32(&mut self) -> Result<i32, FormatError> {
        Ok(i32::from_be_bytes(self.array()?))
    }

    pub(crate) fn u32(&mut self) -> Result<u32, FormatError> {
        Ok(u32::from_be_bytes(self.array()?))
    }

    pub(crate) fn u64(&mut self) -> Result<u64, FormatError> {
        Ok(u64::from_be_bytes(self.array()?))
    }

    pub(crate) fn u128(&mut self) -> Result<u128, FormatError> {
        Ok(u128::from_be_bytes(self.array()?))
    }

    pub(crate) fn f32(&mut self) -> Result<f32, FormatError> {
        Ok(f32::from_be_bytes(self.array()?))
    }

    pub(crate) fn f64(&mut self) -> Result<f64, FormatError> {
        Ok(f64::from_be_bytes(self.array()?))
    }

    /// Succeeds only if everything has been read.
    pub(crate) fn finish(self) -> Result<(), FormatError> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(FormatError::Corrupt("trailing bytes"))
        }
    }
}
