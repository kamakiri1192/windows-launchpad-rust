//! Shared, locale-independent launcher ordering. Labels themselves are untouched.
pub fn key(name: &str, stable_id: &str) -> (String, String) {
    (name.to_lowercase(), stable_id.to_owned())
}
