use super::DomainError;

/// A window into a list: at most `limit` items after skipping `offset`.
///
/// Part of the domain because repositories take it as a query parameter. Building
/// one checks the bounds, so no adapter ever sees an absurd request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Page {
    limit: u64,
    offset: u64,
}

impl Page {
    /// The largest page a caller may ask for.
    pub const MAX_LIMIT: u64 = 100;
    /// The page size used when the caller does not say.
    pub const DEFAULT_LIMIT: u64 = 20;

    /// A page of `limit` items (1 to [`MAX_LIMIT`](Self::MAX_LIMIT)) after `offset`.
    pub fn new(limit: u64, offset: u64) -> Result<Self, DomainError> {
        if (1..=Self::MAX_LIMIT).contains(&limit) {
            Ok(Self { limit, offset })
        } else {
            Err(DomainError::InvalidPage { max: Self::MAX_LIMIT })
        }
    }

    /// Maximum number of items.
    pub fn limit(&self) -> u64 {
        self.limit
    }

    /// Items to skip.
    pub fn offset(&self) -> u64 {
        self.offset
    }

    /// Applies the window to an already sorted list (for in-memory adapters).
    pub fn apply<T>(&self, items: Vec<T>) -> Vec<T> {
        items
            .into_iter()
            .skip(self.offset as usize)
            .take(self.limit as usize)
            .collect()
    }
}

impl Default for Page {
    fn default() -> Self {
        Self {
            limit: Self::DEFAULT_LIMIT,
            offset: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_are_bounded() {
        assert!(Page::new(1, 0).is_ok());
        assert!(Page::new(Page::MAX_LIMIT, 5).is_ok());
        assert_eq!(Page::new(0, 0), Err(DomainError::InvalidPage { max: 100 }));
        assert!(Page::new(101, 0).is_err());
    }

    #[test]
    fn applying_a_page_skips_then_takes() {
        let page = Page::new(2, 1).unwrap();
        assert_eq!(page.apply(vec![1, 2, 3, 4]), vec![2, 3]);
        assert_eq!(page.apply(vec![1]), Vec::<i32>::new());
    }
}
