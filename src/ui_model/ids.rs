#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UiId(String);

impl UiId {
    fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn launcher_item(key: impl AsRef<str>) -> Self {
        Self::new(format!("launcher-item:{}", key.as_ref()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::UiId;
    use std::collections::{BTreeSet, HashSet};

    #[test]
    fn same_string_ids_are_equal() {
        assert_eq!(UiId::launcher_item("calc"), UiId::launcher_item("calc"));
    }

    #[test]
    fn ids_can_be_used_as_stable_btree_set_keys() {
        let mut ids = BTreeSet::new();

        ids.insert(UiId::launcher_item("b"));
        ids.insert(UiId::launcher_item("a"));
        ids.insert(UiId::launcher_item("b"));

        assert_eq!(ids.len(), 2);
        assert_eq!(
            ids.into_iter()
                .map(|id| id.as_str().to_owned())
                .collect::<Vec<_>>(),
            vec!["launcher-item:a".to_owned(), "launcher-item:b".to_owned()]
        );
    }

    #[test]
    fn ids_can_be_used_as_stable_hash_set_keys() {
        let mut ids = HashSet::new();

        ids.insert(UiId::launcher_item("work"));
        ids.insert(UiId::launcher_item("work"));
        ids.insert(UiId::launcher_item("games"));

        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&UiId::launcher_item("work")));
        assert!(ids.contains(&UiId::launcher_item("games")));
    }

    #[test]
    fn as_str_returns_inner_identity() {
        let id = UiId::launcher_item("calc");

        assert_eq!(id.as_str(), "launcher-item:calc");
    }

    #[test]
    fn launcher_item_returns_stable_identity() {
        assert_eq!(UiId::launcher_item("calc"), UiId::launcher_item("calc"));
    }
}
