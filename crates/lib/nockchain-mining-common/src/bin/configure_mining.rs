//! Configure the node's mining payout and enable mining with the operator
//! credential. Restricted miner processes must not receive this credential.

use nockapp::wire::WireRepr;
use nockchain_mining_common::{MiningPkhConfig, NodeClient};

fn usage() -> &'static str {
    "usage: configure_mining <node-http-url> <ai|zk> <share,pkh> [<share,pkh> ...]"
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let url = args.next().ok_or_else(|| usage().to_string())?;
    let source = match args.next().as_deref() {
        Some("ai") => "ai-pow-miner",
        Some("zk") => "zk-pow-miner",
        _ => return Err(usage().into()),
    };
    let configs = args
        .map(|arg| arg.parse::<MiningPkhConfig>())
        .collect::<Result<Vec<_>, _>>()?;
    if configs.is_empty()
        || configs
            .iter()
            .any(|config| config.share == 0 || config.pkh.trim().is_empty())
    {
        return Err(
            "at least one nonempty share,pkh pair with a positive share is required".into(),
        );
    }

    let mut client = NodeClient::connect_operator(url).await?;
    client
        .set_mining_key(
            WireRepr::new(source, 1, vec!["setpubkey".into()]),
            Vec::new(),
            configs,
        )
        .await?;
    client
        .enable_mining(WireRepr::new(source, 1, vec!["enable".into()]), true)
        .await?;
    println!("Mining payout configured and mining enabled on the node");
    Ok(())
}
