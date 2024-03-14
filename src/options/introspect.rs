use clap::Parser;
use reqwest::Url;
use serde::{Deserialize, Serialize};

use crate::{
    options::{OutputOpts, RoverPrinter},
    utils::parsers::parse_header,
    RoverOutput, RoverResult,
};

#[derive(Debug, Serialize, Deserialize, Parser)]
pub struct IntrospectOpts {
    /// The endpoint of the subgraph to introspect
    #[serde(skip_serializing)]
    pub endpoint: Url,

    /// headers to pass to the endpoint. Values must be key:value pairs.
    /// If a value has a space in it, use quotes around the pair,
    /// ex. -H "Auth:some key"

    // The `value_name` here is for the help text and error messages, to print like
    // --header <KEY:VALUE> rather than the plural field name --header <headers>
    #[arg(value_name="KEY:VALUE", long="header", short='H', value_parser = parse_header)]
    #[serde(skip_serializing)]
    pub headers: Option<Vec<(String, String)>>,

    /// poll the endpoint, printing the introspection result if/when its contents change
    #[arg(long)]
    pub watch: bool,
}

impl IntrospectOpts {
    pub async fn exec_and_watch<F>(&self, exec_fn: F, output_opts: &OutputOpts) -> !
    where
        F: Fn() -> impl Future<Output = RoverResult<String>>,
    {
        let mut last_result = None;
        loop {
            match exec_fn().await {
                Ok(sdl) => {
                    let mut was_updated = true;
                    if let Some(last) = last_result {
                        if last == sdl {
                            was_updated = false
                        }
                    }

                    if was_updated {
                        let output = RoverOutput::Introspection(sdl.to_string());
                        let _ = output.write_or_print(output_opts).map_err(|e| e.print());
                    }
                    last_result = Some(sdl);
                }
                Err(error) => {
                    let mut was_updated = true;
                    let e = error.to_string();
                    if let Some(last) = last_result {
                        if last == e {
                            was_updated = false;
                        }
                    }
                    if was_updated {
                        let _ = error.write_or_print(output_opts).map_err(|e| e.print());
                    }
                    last_result = Some(e);
                }
            }
            std::thread::sleep(std::time::Duration::from_secs(1))
        }
    }
}
