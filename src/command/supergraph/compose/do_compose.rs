// TODO: remove once we're no longer using the composition-rewrite feature flag
#[allow(unused_imports)]
use std::{
    env::current_dir,
    fs::File,
    io::{stdin, Read, Write},
    process::Command,
    str,
};

use anyhow::{anyhow, Context};

// TODO: remove once we're no longer using the composition-rewrite feature flag
#[cfg(not(feature = "dev-next"))]
use apollo_federation_types::config::FederationVersion::LatestFedTwo;
use apollo_federation_types::{
    config::{FederationVersion, PluginVersion, SupergraphConfig},
    rover::BuildResult,
};
use camino::Utf8PathBuf;
use clap::{Args, Parser};
use derive_getters::Getters;
use rover_client::{shared::GraphRef, RoverClientError};
use rover_std::warnln;
use semver::Version;
use serde::Serialize;

// TODO: remove once we're no longer using the composition-rewrite feature flag
#[allow(unused_imports)]
use tempfile::tempdir;

// TODO: remove once we're no longer using the composition-rewrite feature flag
#[allow(unused_imports)]
use crate::{
    command::{
        install::{Install, Plugin},
        supergraph::compose::CompositionOutput,
    },
    composition::{
        events::CompositionEvent,
        runner::Runner,
        supergraph::{
            binary::{OutputTarget, SupergraphBinary},
            config::{
                resolve::{
                    subgraph::FullyResolvedSubgraph, FullyResolvedSubgraphs,
                    FullyResolvedSupergraphConfig, UnresolvedSupergraphConfig,
                },
                SupergraphConfigResolver,
            },
            install::InstallSupergraph,
            version::SupergraphVersion,
        },
    },
    options::PluginOpts,
    utils::{
        client::StudioClientConfig,
        effect::{
            exec::TokioCommand,
            fetch_remote_subgraph::RemoteSubgraph,
            install::InstallBinary,
            read_file::FsReadFile,
            write_file::{FsWriteFile, WriteFile},
        },
        expansion::expand,
        parsers::FileDescriptorType,
        supergraph_config::{expand_supergraph_yaml, get_supergraph_config, RemoteSubgraphs},
    },
    RoverError, RoverErrorSuggestion, RoverOutput, RoverResult,
};

#[derive(Debug, Serialize, Parser)]
pub struct Compose {
    #[clap(flatten)]
    opts: SupergraphComposeOpts,
}

#[cfg_attr(test, derive(Default))]
#[derive(Clone, Args, Debug, Serialize, Getters)]
#[group(required = true)]
pub struct SupergraphConfigSource {
    /// The relative path to the supergraph configuration file. You can pass `-` to use stdin instead of a file.
    #[serde(skip_serializing)]
    #[arg(long = "config")]
    supergraph_yaml: Option<FileDescriptorType>,

    /// A [`GraphRef`] that is accessible in Apollo Studio.
    /// This is used to initialize your supergraph with the values contained in this variant.
    ///
    /// This is analogous to providing a supergraph.yaml file with references to your graph variant in studio.
    ///
    /// If used in conjunction with `--config`, the values presented in the supergraph.yaml will take precedence over these values.
    #[arg(long = "graph-ref")]
    graph_ref: Option<GraphRef>,
}

#[cfg_attr(test, derive(Default))]
#[derive(Clone, Debug, Serialize, Parser, Getters)]
pub struct SupergraphComposeOpts {
    #[clap(flatten)]
    pub plugin_opts: PluginOpts,

    #[clap(flatten)]
    pub supergraph_config_source: SupergraphConfigSource,

    /// The version of Apollo Federation to use for composition. If no version is supplied, Rover
    /// will automatically determine the version from the supergraph config
    #[arg(long = "federation-version")]
    federation_version: Option<FederationVersion>,
}

impl Compose {
    #[cfg(not(feature = "dev-next"))]
    pub fn new(compose_opts: PluginOpts) -> Self {
        Self {
            opts: SupergraphComposeOpts {
                plugin_opts: compose_opts,
                federation_version: Some(LatestFedTwo),
                supergraph_config_source: SupergraphConfigSource {
                    supergraph_yaml: Some(FileDescriptorType::File("RAM".into())),
                    graph_ref: None,
                },
            },
        }
    }

    pub(crate) async fn maybe_install_supergraph(
        &self,
        override_install_path: Option<Utf8PathBuf>,
        client_config: StudioClientConfig,
        federation_version: FederationVersion,
    ) -> RoverResult<Utf8PathBuf> {
        let plugin = Plugin::Supergraph(federation_version.clone());
        if federation_version.is_fed_two() {
            self.opts
                .plugin_opts
                .elv2_license_accepter
                .require_elv2_license(&client_config)?;
        }

        // and create our plugin that we may need to install from it
        let install_command = Install {
            force: false,
            plugin: Some(plugin),
            elv2_license_accepter: self.opts.plugin_opts.elv2_license_accepter,
        };

        // maybe do the install, maybe find a pre-existing installation, maybe fail
        let plugin_exe = install_command
            .get_versioned_plugin(
                override_install_path,
                client_config,
                self.opts.plugin_opts.skip_update,
            )
            .await?;
        Ok(plugin_exe)
    }

    #[cfg(feature = "composition-rewrite")]
    pub async fn run(
        &self,
        override_install_path: Option<Utf8PathBuf>,
        client_config: StudioClientConfig,
        output_file: Option<Utf8PathBuf>,
    ) -> RoverResult<RoverOutput> {
        tracing::warn!("using composition-rewrite");
        let mut stdin = stdin();
        let write_file = FsWriteFile::default();
        let read_file = FsReadFile::default();
        let exec_command = TokioCommand::default();

        let studio_client =
            client_config.get_authenticated_client(&self.opts.plugin_opts.profile.clone())?;

        let supergraph_root = self
            .opts
            .clone()
            .supergraph_config_source()
            .clone()
            .supergraph_yaml
            .and_then(|file| match file {
                FileDescriptorType::File(file) => {
                    let mut current_dir =
                        current_dir().expect("Unable to get current directory path");

                    current_dir.push(file);
                    let path = Utf8PathBuf::from_path_buf(current_dir).unwrap();
                    let parent = path.parent().unwrap().to_path_buf();
                    Some(parent)
                }
                FileDescriptorType::Stdin => None,
            });

        // Get a FullyResolvedSupergraphConfig from first loading in any remote subgraphs and then
        // a local supergraph config (if present) and then combining them into a fully resolved
        // supergraph config
        let resolver = SupergraphConfigResolver::default()
            .load_remote_subgraphs(
                &studio_client,
                self.opts.supergraph_config_source.graph_ref.as_ref(),
            )
            .await?
            .load_from_file_descriptor(
                &mut stdin,
                self.opts.supergraph_config_source.supergraph_yaml.as_ref(),
            )?
            .fully_resolve_subgraphs(&client_config, &studio_client, supergraph_root.as_ref())
            .await?;

        // We convert the FullyResolvedSupergraphConfig into a Supergraph because it makes using
        // Serde easier (said differently: we're using the Federation-rs types here for
        // compatability with Federation-rs tooling later on when we use their supergraph binary to
        // actually run composition)
        let supergraph_config: SupergraphConfig = resolver.clone().try_into()?;

        // Convert the FullyResolvedSupergraphConfig to yaml before we save it
        let supergraph_config_yaml = serde_yaml::to_string(&supergraph_config)?;

        // We're going to save to a temporary place because we don't actually need the supergraph
        // config to stick around; we only need it on disk to point the supergraph binary at
        let supergraph_config_filepath =
            Utf8PathBuf::from_path_buf(tempdir()?.path().join("supergraph.yaml"))
                .expect("Unable to parse path");

        // Write the supergraph config to disk
        let _ = write_file
            .write_file(
                &supergraph_config_filepath,
                supergraph_config_yaml.as_bytes(),
            )
            .await?;

        // Use the CLI option for federation over the one we can read off of the supergraph config
        // (but default to the one we can read off the supergraph config)
        let fed_version = self
            .opts
            .federation_version
            .as_ref()
            .unwrap_or(resolver.federation_version());

        // We care about the exact version of the federation version because certain options aren't
        // available before 2.9.0 and we gate on that version below
        let exact_version = fed_version
            .get_exact()
            // This should be impossible to get to because we convert to a FederationVersion a few
            // lines above and so _should_ have an exact version
            .ok_or(RoverError::new(anyhow!(
                "failed to get exact Federation version"
            )))?;

        // Making the output file mutable allows us to change it if we're using a version of the
        // supergraph binary that can't write to file (ie, anything pre-2.9.0)
        let mut output_file = output_file;

        // When the `--output` flag is used, we need a supergraph binary version that is at least
        // v2.9.0. We ignore that flag for composition when we have anything less than that
        if output_file.is_some()
            && (exact_version.major < 2 || (exact_version.major == 2 && exact_version.minor < 9))
        {
            warnln!("ignoring `--output` because it is not supported in this version of the dependent binary, `supergraph`: {}. Upgrade to Federation 2.9.0 or greater to install a version of the binary that supports it.", fed_version);
            output_file = None;
        }

        // Build the supergraph binary, paying special attention to the CLI options
        let supergraph_binary = InstallSupergraph::new(fed_version.clone(), client_config.clone())
            .install(
                override_install_path,
                self.opts.plugin_opts.elv2_license_accepter,
                self.opts.plugin_opts.skip_update,
            )
            .await?;

        let result = supergraph_binary
            .compose(
                &exec_command,
                &read_file,
                &output_file
                    .map(|path| OutputTarget::File(path))
                    .unwrap_or_else(|| OutputTarget::Stdout),
                supergraph_config_filepath,
            )
            .await?;

        Ok(RoverOutput::CompositionResult(result.into()))
    }

    #[cfg(not(feature = "composition-rewrite"))]
    pub async fn run(
        &self,
        override_install_path: Option<Utf8PathBuf>,
        client_config: StudioClientConfig,
        output_file: Option<Utf8PathBuf>,
    ) -> RoverResult<RoverOutput> {
        let mut supergraph_config = get_supergraph_config(
            &self.opts.supergraph_config_source.graph_ref,
            &self.opts.supergraph_config_source.supergraph_yaml.clone(),
            self.opts.federation_version.as_ref(),
            client_config.clone(),
            &self.opts.plugin_opts.profile,
            true,
        )
        .await?
        .ok_or_else(|| anyhow!("error getting supergraph config"))?;

        self.compose(
            override_install_path,
            client_config,
            &mut supergraph_config,
            output_file,
        )
        .await
    }

    pub async fn compose(
        &self,
        override_install_path: Option<Utf8PathBuf>,
        client_config: StudioClientConfig,
        supergraph_config: &mut SupergraphConfig,
        output_file: Option<Utf8PathBuf>,
    ) -> RoverResult<RoverOutput> {
        let output = self
            .exec(
                override_install_path,
                client_config,
                supergraph_config,
                output_file,
            )
            .await?;
        Ok(RoverOutput::CompositionResult(output))
    }

    pub async fn exec(
        &self,
        override_install_path: Option<Utf8PathBuf>,
        client_config: StudioClientConfig,
        supergraph_config: &mut SupergraphConfig,
        output_file: Option<Utf8PathBuf>,
    ) -> RoverResult<CompositionOutput> {
        let mut output_file = output_file;
        // first, grab the _actual_ federation version from the config we just resolved
        // (this will always be `Some` as long as we have created with `resolve_supergraph_yaml` so it is safe to unwrap)
        let federation_version = supergraph_config.get_federation_version().unwrap();

        let exe = self
            .maybe_install_supergraph(
                override_install_path,
                client_config,
                federation_version.clone(),
            )
            .await?;

        // _then_, overwrite the federation_version with _only_ the major version
        // before sending it to the supergraph plugin.
        // we do this because the supergraph binaries _only_ check if the major version is correct
        // and we may want to introduce other semver things in the future.
        // this technique gives us forward _and_ backward compatibility
        // because the supergraph plugin itself only has to parse "federation_version: 1" or "federation_version: 2"
        let v = match federation_version.get_major_version() {
            0 | 1 => FederationVersion::LatestFedOne,
            2 => FederationVersion::LatestFedTwo,
            _ => unreachable!("This version of Rover does not support major versions of federation other than 1 and 2.")
        };
        supergraph_config.set_federation_version(v);
        let num_subgraphs = supergraph_config.get_subgraph_definitions()?.len();
        let supergraph_config_yaml = serde_yaml::to_string(&supergraph_config)?;
        let dir = tempfile::Builder::new().prefix("supergraph").tempdir()?;
        tracing::debug!("temp dir created at {}", dir.path().display());
        let yaml_path = Utf8PathBuf::try_from(dir.path().join("config.yml"))?;
        let mut f = File::create(&yaml_path)?;
        f.write_all(supergraph_config_yaml.as_bytes())?;
        f.sync_all()?;
        tracing::debug!("config file written to {}", &yaml_path);

        let federation_version = Self::extract_federation_version(&exe)?;
        let exact_version = federation_version
            .get_exact()
            // This should be impossible to get to because we convert to a FederationVersion a few
            // lines above and so _should_ have an exact version
            .ok_or(RoverError::new(anyhow!(
                "failed to get exact Federation version"
            )))?;

        eprintln!(
            "composing supergraph with Federation {}",
            &federation_version.get_tarball_version()
        );

        // When the `--output` flag is used, we need a supergraph binary version that is at least
        // v2.9.0. We ignore that flag for composition when we have anything less than that
        if output_file.is_some()
            && (exact_version.major < 2 || (exact_version.major == 2 && exact_version.minor < 9))
        {
            warnln!("ignoring `--output` because it is not supported in this version of the dependent binary, `supergraph`: {}. Upgrade to Federation 2.9.0 or greater to install a version of the binary that supports it.", federation_version);
            output_file = None;
        }

        // Whether we use stdout or a file dependson whether the the `--output` option was used
        let content = match output_file {
            // If it was, we use a file in the supergraph binary; this cuts down the overall time
            // it takes to do composition when we're working on really large compositions, but it
            // carries with it the assumption that stdout is superfluous
            Some(filepath) => {
                Command::new(&exe)
                    .args(["compose", yaml_path.as_ref(), filepath.as_ref()])
                    .output()
                    .context("Failed to execute command")?;

                let mut composition_file = std::fs::File::open(&filepath).unwrap();
                let mut content: String = String::new();
                composition_file.read_to_string(&mut content).unwrap();
                content
            }
            // When we aren't using `--output`, we dump the composition directly to stdout
            None => {
                let output = Command::new(&exe)
                    .args(["compose", yaml_path.as_ref()])
                    .output()
                    .context("Failed to execute command")?;

                let content = str::from_utf8(&output.stdout)
                    .with_context(|| format!("Could not parse output of `{} compose`", &exe))?;
                content.to_string()
            }
        };

        // Make sure the composition is well-formed
        let composition = match serde_json::from_str::<BuildResult>(&content) {
            Ok(res) => res,
            Err(err) => {
                return Err(anyhow!("{}", err))
                    .with_context(|| anyhow!("{} compose output: {}", &exe, content))
                    .with_context(|| anyhow!("Output from `{} compose` was malformed.", &exe))
                    .map_err(|e| {
                        let mut error = RoverError::new(e);
                        error.set_suggestion(RoverErrorSuggestion::SubmitIssue);
                        error
                    })
            }
        };

        match composition {
            Ok(build_output) => Ok(CompositionOutput {
                hints: build_output.hints,
                supergraph_sdl: build_output.supergraph_sdl,
                federation_version: Some(format_version(federation_version.to_string())),
            }),
            Err(build_errors) => Err(RoverError::from(RoverClientError::BuildErrors {
                source: build_errors,
                num_subgraphs,
            })),
        }
    }

    /// Extracts the Federation Version from the executable
    fn extract_federation_version(exe: &Utf8PathBuf) -> Result<FederationVersion, RoverError> {
        let file_name = exe.file_name().unwrap();
        let without_exe = file_name.strip_suffix(".exe").unwrap_or(file_name);
        let version = match Version::parse(
            without_exe
                .strip_prefix("supergraph-v")
                .unwrap_or(without_exe),
        ) {
            Ok(version) => version,
            Err(err) => return Err(RoverError::new(err)),
        };

        match version.major {
            0 | 1 => Ok(FederationVersion::ExactFedOne(version)),
            2 => Ok(FederationVersion::ExactFedTwo(version)),
            _ => Err(RoverError::new(anyhow!("unsupported Federation version"))),
        }
    }
}

/// Format the a Version string (coming from an exact version, which includes a `=` rather than a
/// `v`) for readability
fn format_version(version: String) -> String {
    let unformatted = &version[1..];
    let mut formatted = unformatted.to_string();
    formatted.insert(0, 'v');
    formatted
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use speculoos::assert_that;

    use super::*;

    #[rstest]
    #[case::simple_binary("a/b/c/d/supergraph-v2.8.5", "v2.8.5")]
    #[case::simple_windows_binary("a/b/supergraph-v2.9.1.exe", "v2.9.1")]
    #[case::complicated_semver(
        "a/b/supergraph-v1.2.3-SNAPSHOT.123+asdf",
        "v1.2.3-SNAPSHOT.123+asdf"
    )]
    #[case::complicated_semver_windows(
        "a/b/supergraph-v1.2.3-SNAPSHOT.123+asdf.exe",
        "v1.2.3-SNAPSHOT.123+asdf"
    )]
    fn it_can_extract_a_version_correctly(#[case] file_path: &str, #[case] expected_value: &str) {
        let mut fake_path = Utf8PathBuf::new();
        fake_path.push(file_path);
        let result = Compose::extract_federation_version(&fake_path).unwrap();
        assert_that(&result).matches(|f| format_version(f.to_string()) == expected_value);
    }
}
