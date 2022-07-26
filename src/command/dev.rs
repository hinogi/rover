use crate::command::subgraph::{Dev as SubgraphDev, SubgraphDevOpts};
use crate::command::RoverOutput;
use crate::utils::client::StudioClientConfig;
use crate::Result;
use saucer::{clap, Parser, Utf8PathBuf};

use serde::Serialize;

#[derive(Debug, Serialize, Clone, Parser)]
pub struct Dev {
    #[clap(flatten)]
    opts: SubgraphDevOpts,
}

impl Dev {
    #[cfg(feature = "composition-js")]
    pub fn run(
        &self,
        override_install_path: Option<Utf8PathBuf>,
        client_config: StudioClientConfig,
    ) -> Result<RoverOutput> {
        // TODO: ensure project is subgraph type before running

        SubgraphDev {
            opts: self.opts.clone(),
        }
        .run(override_install_path, client_config)
    }

    #[cfg(not(feature = "composition-js"))]
    pub fn run(
        &self,
        override_install_path: Option<Utf8PathBuf>,
        client_config: StudioClientConfig,
    ) -> Result<RoverOutput> {
        SubgraphDev {
            opts: self.opts.clone(),
        }
        .run(override_install_path, client_config)
    }
}