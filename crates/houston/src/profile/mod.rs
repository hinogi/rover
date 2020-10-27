mod sensitive;

use crate::{home, HoustonProblem};
use sensitive::Sensitive;
use serde::{Deserialize, Serialize};

use std::env;
use std::fmt;
use std::fs;
use std::path::PathBuf;

/// Collects configuration related to a profile.
#[derive(Debug, Serialize, Deserialize)]
pub struct Profile {
    sensitive: Sensitive,
}

/// Represents all possible options in loading configuration
pub struct LoadOpts {
    /// Should sensitive config be included in the load
    pub sensitive: bool,
}

/// Represents all possible configuration options.
pub struct Opts {
    /// Apollo API Key
    pub api_key: Option<String>,
}

impl Profile {
    fn dir(name: &str) -> Result<PathBuf, HoustonProblem> {
        Ok(home::dir()?.join("profiles").join(name))
    }

    /// Writes an api_key to the filesystem (`$APOLLO_CONFIG_HOME/profiles/<profile_name>/.sensitive`).
    pub fn set_api_key(name: &str, api_key: String) -> Result<(), HoustonProblem> {
        let opts = Opts {
            api_key: Some(api_key),
        };
        Profile::save(name, opts)?;
        Ok(())
    }

    /// Returns an API key for interacting with Apollo services.
    ///
    /// Checks for the presence of an `APOLLO_KEY` env var, and returns its value
    /// if it finds it. Otherwise looks for credentials on the file system.
    ///
    /// Takes an optional `profile` argument. Defaults to `"default"`.
    pub fn get_api_key(name: &str) -> Result<String, HoustonProblem> {
        let apollo_key = env::var("APOLLO_KEY");
        tracing::debug!(APOLLO_KEY = ?apollo_key);
        match apollo_key.ok() {
            Some(api_key) => Ok(api_key),
            None => {
                let opts = LoadOpts { sensitive: true };
                Ok(Profile::load(name, opts)?.sensitive.api_key)
            }
        }
    }

    /// Saves configuration options for a specific profile to the file system,
    /// splitting sensitive information into a separate file.
    pub fn save(name: &str, opts: Opts) -> Result<(), HoustonProblem> {
        if let Some(api_key) = opts.api_key {
            Sensitive { api_key }.save(name)?;
        }
        Ok(())
    }

    /// Loads and deserializes configuration from the file system for a
    /// specific profile.
    pub fn load(name: &str, opts: LoadOpts) -> Result<Profile, HoustonProblem> {
        if Profile::dir(name)?.exists() {
            if opts.sensitive {
                let sensitive = Sensitive::load(name)?;
                return Ok(Profile { sensitive });
            }
            Err(HoustonProblem::NoNonSensitiveConfigFound(name.to_string()))
        } else {
            Err(HoustonProblem::ProfileNotFound(name.to_string()))
        }
    }

    /// Deletes profile data from file system.
    pub fn delete(name: &str) -> Result<(), HoustonProblem> {
        let dir = Profile::dir(name)?;
        tracing::debug!(dir = ?dir);
        Ok(fs::remove_dir_all(dir)?)
    }

    /// Lists profiles based on directories in `$APOLLO_CONFIG_HOME/profiles`
    pub fn list() -> Result<Vec<String>, HoustonProblem> {
        let profiles_dir = home::dir()?.join("profiles");
        let mut profiles = vec![];

        // if profiles dir doesn't exist return empty vec
        let entries = fs::read_dir(profiles_dir);

        if let Ok(entries) = entries {
            for entry in entries {
                let entry_path = entry?.path();
                if entry_path.is_dir() {
                    let profile = entry_path.file_stem().unwrap();
                    tracing::debug!(profile = ?profile);
                    profiles.push(profile.to_string_lossy().into_owned());
                }
            }
        }
        Ok(profiles)
    }
}

impl fmt::Display for Profile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.sensitive)
    }
}