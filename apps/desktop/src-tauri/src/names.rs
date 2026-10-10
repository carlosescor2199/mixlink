//! Musician labels, persisted in the app's config directory.
//!
//! The label is a desktop-app concern only: the engine never sees it, it is not part of any
//! protocol and it is never sent to a client. The store maps the exact address string the engine
//! reports to the name the engineer typed, so the UI can show a name instead of an address and the
//! mapping survives a restart.
//!
//! The file is a small JSON object, `{"192.168.1.30:50000": "Ana"}`. A missing or unreadable file
//! is treated as an empty store rather than an error: labels are convenience, and losing them must
//! not stop the server from starting.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

/// The file name inside the app config directory.
pub const NAMES_FILE: &str = "musicians.json";

/// The address-to-name map, backed by a JSON file.
pub struct MusicianNames {
    path: PathBuf,
    names: Mutex<BTreeMap<String, String>>,
}

impl MusicianNames {
    /// Loads the store from `path`, treating a missing or unreadable file as empty.
    pub fn load(path: PathBuf) -> Self {
        let names = match fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<BTreeMap<String, String>>(&text) {
                Ok(names) => names,
                Err(error) => {
                    eprintln!(
                        "[mixlink-desktop] musician names file {} is not valid JSON ({error}); starting with no labels",
                        path.display()
                    );
                    BTreeMap::new()
                }
            },
            Err(_) => BTreeMap::new(),
        };
        Self {
            path,
            names: Mutex::new(names),
        }
    }

    /// The label for an address, or `None` when it has none.
    pub fn get(&self, address: &str) -> Option<String> {
        self.names
            .lock()
            .expect("musician names lock poisoned")
            .get(address)
            .cloned()
    }

    /// Sets the label for an address. An empty (or whitespace-only) name clears it.
    ///
    /// The file is written before success is reported, so a restart either sees the new label or
    /// the old one, never a half-applied state reported as saved.
    pub fn set(&self, address: &str, name: &str) -> Result<(), String> {
        let mut names = self.names.lock().expect("musician names lock poisoned");
        let name = name.trim();
        if name.is_empty() {
            names.remove(address);
        } else {
            names.insert(address.to_owned(), name.to_owned());
        }
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "could not create the config directory {}: {error}",
                    parent.display()
                )
            })?;
        }
        let json = serde_json::to_string_pretty(&*names)
            .map_err(|error| format!("could not serialize the musician names: {error}"))?;
        fs::write(&self.path, json)
            .map_err(|error| format!("could not write {}: {error}", self.path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_path(test: &str) -> PathBuf {
        let directory = std::env::temp_dir().join("mixlink-names-tests").join(test);
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).expect("test directory should be created");
        directory.join(NAMES_FILE)
    }

    #[test]
    fn a_name_survives_a_reload() {
        let path = test_path("reload");
        let names = MusicianNames::load(path.clone());

        names
            .set("192.168.1.30:50000", "Ana")
            .expect("setting a name should persist");

        let reloaded = MusicianNames::load(path);
        assert_eq!(reloaded.get("192.168.1.30:50000"), Some("Ana".to_owned()));
    }

    #[test]
    fn an_empty_name_clears_the_label() {
        let path = test_path("clear");
        let names = MusicianNames::load(path.clone());
        names.set("192.168.1.30:50000", "Ana").expect("set");

        names.set("192.168.1.30:50000", "   ").expect("clear");

        assert_eq!(names.get("192.168.1.30:50000"), None);
        assert_eq!(MusicianNames::load(path).get("192.168.1.30:50000"), None);
    }

    #[test]
    fn renaming_one_musician_leaves_the_others_alone() {
        let path = test_path("rename");
        let names = MusicianNames::load(path.clone());
        names.set("192.168.1.30:50000", "Ana").expect("set");
        names.set("192.168.1.31:50000", "Beto").expect("set");

        names
            .set("192.168.1.30:50000", "Ana Maria")
            .expect("rename");

        let reloaded = MusicianNames::load(path);
        assert_eq!(
            reloaded.get("192.168.1.30:50000"),
            Some("Ana Maria".to_owned())
        );
        assert_eq!(reloaded.get("192.168.1.31:50000"), Some("Beto".to_owned()));
    }

    #[test]
    fn a_missing_file_loads_as_an_empty_store() {
        let path = test_path("missing").join("nested").join(NAMES_FILE);

        let names = MusicianNames::load(path.clone());

        assert_eq!(names.get("192.168.1.30:50000"), None);
        names.set("192.168.1.30:50000", "Ana").expect("set");
        assert_eq!(
            MusicianNames::load(path).get("192.168.1.30:50000"),
            Some("Ana".to_owned())
        );
    }

    #[test]
    fn a_corrupt_file_loads_as_an_empty_store_and_is_rewritten_on_the_next_set() {
        let path = test_path("corrupt");
        fs::write(&path, "this is not json").expect("write corrupt file");

        let names = MusicianNames::load(path.clone());
        assert_eq!(names.get("192.168.1.30:50000"), None);

        names.set("192.168.1.30:50000", "Ana").expect("set");

        assert_eq!(
            MusicianNames::load(path).get("192.168.1.30:50000"),
            Some("Ana".to_owned())
        );
    }
}
