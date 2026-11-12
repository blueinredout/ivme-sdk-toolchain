use clap::Args;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::PathBuf;

#[derive(Args)]
pub(crate) struct AuthorReleaseArgs {
    #[command(subcommand)]
    command: AuthorReleaseCommand,
}

#[derive(clap::Subcommand)]
enum AuthorReleaseCommand {
    Sign(SignArgs),
}

#[derive(Args)]
struct SignArgs {
    #[arg(long)]
    intent: PathBuf,
    #[arg(long)]
    package: PathBuf,
    #[arg(long, value_name = "HEX")]
    key_id: String,
    #[arg(long)]
    publisher: String,
    #[arg(long)]
    app_id: String,
    #[arg(long)]
    workshop_item_id: String,
    #[arg(long)]
    output: PathBuf,
    #[arg(long)]
    directory: Option<PathBuf>,
}

pub(crate) fn sign(args: AuthorReleaseArgs) -> Result<String, String> {
    match args.command {
        AuthorReleaseCommand::Sign(args) => sign_intent(args),
    }
}

fn sign_intent(args: SignArgs) -> Result<String, String> {
    if !crate::author_keys::valid_lower_hex(&args.key_id, 64) {
        return Err("key ID must be 64 lowercase hexadecimal characters".into());
    }
    crate::author_keys::canonical_decimal_u64(&args.publisher)?;
    crate::author_keys::canonical_decimal_u64(&args.workshop_item_id)?;
    let app = args
        .app_id
        .parse::<u32>()
        .map_err(|_| "app ID must be a nonzero uint32")?;
    if app == 0 || app.to_string() != args.app_id {
        return Err("app ID must be a canonical nonzero uint32".into());
    }

    let intent = crate::author_keys::read_bounded(&args.intent, 65_536, "intent file")?;
    let text = std::str::from_utf8(&intent).map_err(|_| "intent is not valid UTF-8")?;
    let claims: Value = serde_json::from_str(text).map_err(|_| "intent JSON is invalid")?;
    if crate::author_release::canonical_body(&claims)
        .map_err(|_| "intent claims are invalid")?
        .as_slice()
        != intent.as_slice()
    {
        return Err("intent is not canonical".into());
    }
    if claims["app_id"].as_u64() != Some(app as u64)
        || claims["publisher_profile_id"].as_str() != Some(args.publisher.as_str())
        || claims["workshop_item_id"].as_str() != Some(args.workshop_item_id.as_str())
        || claims["author_key_id"].as_str() != Some(args.key_id.as_str())
    {
        return Err(
            "intent context does not match the explicit app, publisher, item, and key".into(),
        );
    }
    let package = crate::author_keys::read_bounded(&args.package, 67_108_864, "package file")?;
    let package_sha = hex(&Sha256::digest(&package));
    if claims["package_size"].as_u64() != Some(package.len() as u64)
        || claims["package_sha256"].as_str() != Some(package_sha.as_str())
    {
        return Err("package bytes do not match the signed intent claims".into());
    }

    // Validate destination (including UTF-8 name and no existing file) before opening private.pem.
    crate::author_keys::output_destination(&args.output, "release")?;
    let envelope = crate::author_keys::sign_managed(1, &args.key_id, args.directory, &intent)?;
    let release_path = crate::author_keys::publish_public(&args.output, &envelope, "release")?;
    let release_path = release_path
        .to_str()
        .ok_or("release output path is not valid UTF-8")?;
    serde_json::to_string(&serde_json::json!({
        "schema_version": 1,
        "key_id": args.key_id,
        "submission_id": claims["submission_id"],
        "release_path": release_path,
        "release_sha256": hex(&Sha256::digest(&envelope)),
    }))
    .map_err(|_| "cannot encode release result".into())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
