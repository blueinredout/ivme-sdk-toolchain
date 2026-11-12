use clap::{Args, Parser, Subcommand};
use rsa::{
    pkcs8::{DecodePublicKey, EncodePrivateKey, EncodePublicKey, LineEnding},
    traits::PublicKeyParts,
    Generate, PrivateKey, PublicKey,
};
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    fs::OpenOptions,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    process,
};
use zeroize::Zeroizing;

#[derive(Parser)]
#[command(name = "ivlc", disable_help_subcommand = true)]
struct AuthorKeyCli {
    #[command(subcommand)]
    command: NamespaceCommand,
}

#[derive(Subcommand)]
enum NamespaceCommand {
    #[command(name = "author-key")]
    AuthorKey(AuthorKeyArgs),
    #[command(name = "author-release")]
    AuthorRelease(crate::author_release_cli::AuthorReleaseArgs),
}

#[derive(Args)]
struct AuthorKeyArgs {
    #[command(subcommand)]
    command: AuthorKeyCommand,
}

#[derive(Subcommand)]
enum AuthorKeyCommand {
    Create(CreateArgs),
    Info(InfoArgs),
    Prove(ProveArgs),
}

#[derive(Args)]
struct CreateArgs {
    #[arg(long)]
    directory: Option<PathBuf>,
}
#[derive(Args)]
struct InfoArgs {
    #[arg(long, value_name = "HEX")]
    key_id: String,
    #[arg(long)]
    directory: Option<PathBuf>,
}
#[derive(Args)]
struct ProveArgs {
    #[arg(long)]
    challenge: PathBuf,
    #[arg(long, value_name = "HEX")]
    key_id: String,
    #[arg(long)]
    publisher: String,
    #[arg(long)]
    app_id: String,
    #[arg(long)]
    output: PathBuf,
    #[arg(long)]
    directory: Option<PathBuf>,
}

pub fn dispatch() -> ! {
    let parsed = match AuthorKeyCli::try_parse_from(env::args_os()) {
        Ok(cli) => cli,
        Err(error) => {
            let code = error.exit_code();
            let _ = error.print();
            process::exit(code);
        }
    };
    let result = match parsed.command {
        NamespaceCommand::AuthorRelease(args) => crate::author_release_cli::sign(args),
        NamespaceCommand::AuthorKey(args) => match args.command {
            AuthorKeyCommand::Create(args) => create(args.directory),
            AuthorKeyCommand::Info(args) => info(&args.key_id, args.directory),
            AuthorKeyCommand::Prove(args) => prove(args),
        },
    };
    match result {
        Ok(json) => {
            println!("{json}");
            process::exit(0);
        }
        Err(error) => {
            eprintln!("Error: {error}");
            process::exit(1);
        }
    }
}

fn default_root() -> Result<PathBuf, String> {
    #[cfg(windows)]
    {
        let appdata = env::var_os("APPDATA").ok_or("APPDATA is not set")?;
        let base = PathBuf::from(appdata);
        if !base.is_absolute() {
            return Err("APPDATA must be an absolute user directory".into());
        }
        Ok(base.join("IVME/author-keys/v1"))
    }
    #[cfg(not(windows))]
    {
        if let Some(xdg) = env::var_os("XDG_CONFIG_HOME") {
            let base = PathBuf::from(xdg);
            if !base.is_absolute() {
                return Err("XDG_CONFIG_HOME must be an absolute user directory".into());
            }
            return Ok(base.join("ivme/author-keys/v1"));
        }
        let home = env::var_os("HOME").ok_or("XDG_CONFIG_HOME and HOME are not set")?;
        let base = PathBuf::from(home);
        if !base.is_absolute() {
            return Err("HOME must be an absolute user directory".into());
        }
        Ok(base.join(".config/ivme/author-keys/v1"))
    }
}

fn is_link_or_reparse(path: &Path) -> Result<bool, String> {
    let metadata = fs::symlink_metadata(path).map_err(|e| format!("cannot inspect path: {e}"))?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        Ok(metadata.file_type().is_symlink() || metadata.file_attributes() & 0x400 != 0)
    }
    #[cfg(not(windows))]
    {
        Ok(metadata.file_type().is_symlink())
    }
}

fn safe_root(requested: Option<PathBuf>, create: bool) -> Result<PathBuf, String> {
    let requested = requested.map(Ok).unwrap_or_else(default_root)?;
    if requested.as_os_str().is_empty() {
        return Err("key directory cannot be empty".into());
    }
    let absolute = if requested.is_absolute() {
        requested
    } else {
        env::current_dir()
            .map_err(|e| e.to_string())?
            .join(requested)
    };
    let mut cursor = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => cursor.push(component.as_os_str()),
            Component::CurDir => continue,
            Component::ParentDir => return Err("directory path cannot contain '..'".into()),
            Component::Normal(part) => {
                cursor.push(part);
                match fs::symlink_metadata(&cursor) {
                    Ok(metadata) => {
                        if is_link_or_reparse(&cursor)? || !metadata.is_dir() {
                            return Err(
                                "directory path contains a link or non-directory component".into(),
                            );
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound && create => {
                        create_private_dir(&cursor)?;
                        let metadata = fs::symlink_metadata(&cursor).map_err(|e| e.to_string())?;
                        if is_link_or_reparse(&cursor)? || !metadata.is_dir() {
                            return Err(
                                "directory path contains a link or non-directory component".into(),
                            );
                        }
                    }
                    Err(e) => {
                        return Err(format!(
                            "directory does not exist or cannot be inspected: {e}"
                        ))
                    }
                }
            }
        }
    }
    fs::canonicalize(&absolute).map_err(|e| format!("cannot resolve directory: {e}"))
}

fn create_private_dir(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        builder
            .create(path)
            .map_err(|e| format!("cannot create directory: {e}"))
    }
    #[cfg(not(unix))]
    {
        fs::create_dir(path).map_err(|e| format!("cannot create directory: {e}"))
    }
}

fn create_private_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|e| format!("cannot create key file: {e}"))?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| format!("cannot persist key file: {e}"))
}

fn id_for(key: &PublicKey) -> Result<String, String> {
    let der = key
        .to_public_key_der()
        .map_err(|_| "cannot encode public key".to_string())?;
    Ok(hex(&Sha256::digest(der.as_bytes())))
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn create(directory: Option<PathBuf>) -> Result<String, String> {
    let root = safe_root(directory, true)?;
    if root.to_str().is_none() {
        return Err("key directory is not valid UTF-8".into());
    }
    let private =
        PrivateKey::try_generate().map_err(|_| "RSA key generation failed".to_string())?;
    let public = private.as_public_key();
    let key_id = id_for(&public)?;
    let private_pem = private
        .to_pkcs8_pem(LineEnding::LF)
        .map_err(|_| "private key encoding failed".to_string())?;
    let public_pem = public
        .to_public_key_pem(LineEnding::LF)
        .map_err(|_| "public key encoding failed".to_string())?;
    let destination = root.join(&key_id);
    match fs::symlink_metadata(&destination) {
        Ok(_) => return Err("key directory already exists".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(format!("cannot inspect key directory: {e}")),
    }
    let stage = root.join(format!(".stage-{}", uuid::Uuid::new_v4()));
    create_private_dir(&stage)?;
    let write_result = (|| {
        create_private_file(&stage.join("private.pem"), private_pem.as_bytes())?;
        create_private_file(&stage.join("public.pem"), public_pem.as_bytes())?;
        fs::rename(&stage, &destination)
            .map_err(|e| format!("cannot publish key directory: {e}"))?;
        Ok::<(), String>(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_dir_all(&stage);
    }
    write_result?;
    let dir = destination
        .to_str()
        .ok_or("key directory is not valid UTF-8")?;
    serde_json::to_string(&serde_json::json!({"schema_version": 1, "key_id": key_id, "public_key_pem": public_pem, "key_directory": dir})).map_err(|e| e.to_string())
}

struct ManagedPublicInfo {
    key_id: String,
    pem: String,
    directory: PathBuf,
}

fn managed_public_info(
    key_id: &str,
    directory: Option<PathBuf>,
) -> Result<ManagedPublicInfo, String> {
    if key_id.len() != 64
        || !key_id
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err("key ID must be 64 lowercase hexadecimal characters".into());
    }
    let root = safe_root(directory, false)?;
    let dir = root.join(key_id);
    let metadata =
        fs::symlink_metadata(&dir).map_err(|_| "key directory was not found".to_string())?;
    if is_link_or_reparse(&dir)? || !metadata.is_dir() {
        return Err("key directory is not a regular directory".into());
    }
    let path = dir.join("public.pem");
    let metadata =
        fs::symlink_metadata(&path).map_err(|_| "public key file was not found".to_string())?;
    if is_link_or_reparse(&path)? || !metadata.is_file() || metadata.len() > 2048 {
        return Err("public key file is invalid or exceeds 2048 bytes".into());
    }
    let mut bytes = Vec::with_capacity(2049);
    fs::File::open(&path)
        .map_err(|e| format!("cannot read public key file: {e}"))?
        .take(2049)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("cannot read public key file: {e}"))?;
    if bytes.len() > 2048 {
        return Err("public key file exceeds 2048 bytes".into());
    }
    let pem =
        std::str::from_utf8(&bytes).map_err(|_| "public key PEM is not ASCII UTF-8".to_string())?;
    if !pem.is_ascii() {
        return Err("public key PEM is not ASCII".into());
    }
    let public =
        PublicKey::from_public_key_pem(pem).map_err(|_| "public key PEM is invalid".to_string())?;
    if public.n().bit_width() != 3072 {
        return Err("public key is not RSA-3072".into());
    }
    let actual = id_for(&public)?;
    if actual != key_id {
        return Err("public key does not match the requested key ID".into());
    }
    let dir = fs::canonicalize(dir).map_err(|e| e.to_string())?;
    Ok(ManagedPublicInfo {
        key_id: actual,
        pem: pem.to_owned(),
        directory: dir,
    })
}

fn info(key_id: &str, directory: Option<PathBuf>) -> Result<String, String> {
    let public = managed_public_info(key_id, directory)?;
    let dir = public
        .directory
        .to_str()
        .ok_or("key directory is not valid UTF-8")?;
    serde_json::to_string(&serde_json::json!({"schema_version": 1, "key_id": public.key_id, "public_key_pem": public.pem, "key_directory": dir})).map_err(|e| e.to_string())
}

const MAX_CHALLENGE_BYTES: usize = 4096;
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

pub(crate) fn canonical_decimal_u64(value: &str) -> Result<u64, String> {
    if value.is_empty()
        || !value.bytes().all(|b| b.is_ascii_digit())
        || (value.len() > 1 && value.starts_with('0'))
    {
        return Err("publisher must be a canonical decimal uint64".into());
    }
    let parsed = value
        .parse::<u64>()
        .map_err(|_| "publisher must be a canonical decimal uint64")?;
    if parsed == 0 {
        return Err("publisher must be a nonzero decimal uint64".into());
    }
    Ok(parsed)
}

pub(crate) fn valid_lower_hex(value: &str, len: usize) -> bool {
    value.len() == len
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn canonical_challenge(
    bytes: &[u8],
    key_id: &str,
    publisher: &str,
    app_id: u32,
) -> Result<String, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "challenge is not valid UTF-8")?;
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|_| "challenge JSON is invalid")?;
    let object = value.as_object().ok_or("challenge must be a JSON object")?;
    const FIELDS: [&str; 9] = [
        "schema_version",
        "app_id",
        "publisher_profile_id",
        "author_key_id",
        "challenge_id",
        "nonce",
        "expires_at_unix",
        "operation",
        "replaces_key_id",
    ];
    if object.len() != FIELDS.len() || FIELDS.iter().any(|field| !object.contains_key(*field)) {
        return Err("challenge must contain exactly the nine required fields".into());
    }
    let sorted: std::collections::BTreeMap<_, _> = object.iter().collect();
    if serde_json::to_vec(&sorted).map_err(|_| "challenge JSON is invalid")? != bytes {
        return Err("challenge JSON is not canonical".into());
    }
    if object["schema_version"].as_u64() != Some(1) {
        return Err("unsupported challenge schema version".into());
    }
    if object["app_id"].as_u64() != Some(app_id as u64) || app_id == 0 {
        return Err("challenge app ID does not match --app-id".into());
    }
    let profile = object["publisher_profile_id"]
        .as_str()
        .ok_or("publisher profile ID must be a decimal string")?;
    if canonical_decimal_u64(profile)? != canonical_decimal_u64(publisher)? || profile != publisher
    {
        return Err("challenge publisher does not match --publisher".into());
    }
    let challenge_key = object["author_key_id"]
        .as_str()
        .ok_or("author key ID must be a string")?;
    if !valid_lower_hex(challenge_key, 64) || challenge_key != key_id {
        return Err("challenge author key ID does not match --key-id".into());
    }
    let challenge_id = object["challenge_id"]
        .as_str()
        .ok_or("challenge ID must be a string")?;
    let parsed_uuid = uuid::Uuid::parse_str(challenge_id)
        .map_err(|_| "challenge ID must be a canonical lowercase UUID")?;
    if parsed_uuid.to_string() != challenge_id {
        return Err("challenge ID must be a canonical lowercase UUID".into());
    }
    let nonce = object["nonce"]
        .as_str()
        .ok_or("challenge nonce must be a string")?;
    if !valid_lower_hex(nonce, 64) {
        return Err("challenge nonce must be 64 lowercase hexadecimal characters".into());
    }
    let expires = object["expires_at_unix"]
        .as_u64()
        .ok_or("challenge expiry must be a positive integer")?;
    if expires == 0 || expires > MAX_SAFE_INTEGER {
        return Err("challenge expiry is outside the supported integer range".into());
    }
    let operation = object["operation"]
        .as_str()
        .ok_or("challenge operation must be a string")?;
    let replaces = object["replaces_key_id"]
        .as_str()
        .ok_or("predecessor key ID must be a string")?;
    match operation {
        "bind" if replaces.is_empty() => (),
        "rotate" | "recover" if valid_lower_hex(replaces, 64) && replaces != key_id => (),
        "bind" | "rotate" | "recover" => {
            return Err("challenge predecessor does not match its operation".into())
        }
        _ => return Err("unsupported challenge operation".into()),
    }
    Ok(challenge_id.to_owned())
}

pub(crate) fn read_bounded(
    path: &Path,
    limit: usize,
    description: &str,
) -> Result<Zeroizing<Vec<u8>>, String> {
    let mut file = fs::File::open(path).map_err(|e| format!("cannot read {description}: {e}"))?;
    let metadata = file
        .metadata()
        .map_err(|e| format!("cannot inspect {description}: {e}"))?;
    if !metadata.is_file() || metadata.len() > limit as u64 {
        return Err(format!("{description} is not a bounded regular file"));
    }
    let mut bytes = Zeroizing::new(Vec::with_capacity(limit.min(metadata.len() as usize) + 1));
    Read::by_ref(&mut file)
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("cannot read {description}: {e}"))?;
    if bytes.len() > limit {
        return Err(format!("{description} exceeds {limit} bytes"));
    }
    Ok(bytes)
}

pub(crate) fn sign_managed(
    kind: u8, key_id: &str, directory: Option<PathBuf>, body: &[u8],
) -> Result<Vec<u8>, String> {
    let public = managed_public_info(key_id, directory)?;
    if !matches!(kind, 1 | 3) { return Err("unsupported author signature kind".into()); }
    let private_path = public.directory.join("private.pem");
    if is_link_or_reparse(&private_path).map_err(|_| "managed private key cannot be inspected")? {
        return Err("managed private key must be a regular file".into());
    }
    let private_bytes = read_bounded(&private_path, 4096, "managed private key")?;
    let private_pem = std::str::from_utf8(&private_bytes)
        .map_err(|_| "managed private key is not valid UTF-8")?;
    let envelope = crate::signed_envelope::sign(kind, body, private_pem)
        .map_err(|_| "managed private key could not sign the request")?;
    let verified = crate::signed_envelope::verify(&envelope, &public.pem, kind)
        .map_err(|_| "managed private key does not match its public key")?;
    if verified.body != body || hex(&verified.key_id) != key_id {
        return Err("managed private key does not match its public key".into());
    }
    Ok(envelope)
}

pub(crate) fn output_destination(output: &Path, label: &str) -> Result<PathBuf, String> {
    let requested = if output.is_absolute() { output.to_path_buf() } else {
        env::current_dir().map_err(|e| e.to_string())?.join(output)
    };
    let file_name = requested.file_name().ok_or("output must name a new file")?;
    let parent = requested.parent().ok_or("output must name a new file")?;
    let parent = fs::canonicalize(parent).map_err(|e| format!("output directory cannot be resolved: {e}"))?;
    let destination = parent.join(file_name);
    if destination.to_str().is_none() { return Err(format!("{label} output path is not valid UTF-8")); }
    match fs::symlink_metadata(&destination) {
        Ok(_) => return Err(format!("{label} output already exists")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(format!("{label} output cannot be inspected: {e}")),
    }
    Ok(destination)
}

pub(crate) fn publish_public(
    output: &Path, bytes: &[u8], label: &str,
) -> Result<PathBuf, String> {
    let destination = output_destination(output, label)?;
    let parent = destination.parent().ok_or("output must name a new file")?;
    let temp = parent.join(format!(".ivlc-{label}-{}", uuid::Uuid::new_v4()));
    let mut owns_temp = false;
    let result = (|| {
        let mut options = OpenOptions::new(); options.write(true).create_new(true);
        #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
        let mut file = options.open(&temp).map_err(|e| format!("cannot create {label} staging file: {e}"))?;
        owns_temp = true;
        file.write_all(bytes).and_then(|_| file.sync_all()).map_err(|e| format!("cannot persist {label}: {e}"))?;
        fs::hard_link(&temp, &destination).map_err(|_| format!("{label} output already exists or cannot be published atomically"))?;
        Ok::<(), String>(())
    })();
    if owns_temp { let _ = fs::remove_file(&temp); }
    result?;
    Ok(destination)
}

fn prove(args: ProveArgs) -> Result<String, String> {
    if !valid_lower_hex(&args.key_id, 64) {
        return Err("key ID must be 64 lowercase hexadecimal characters".into());
    }
    canonical_decimal_u64(&args.publisher)?;
    let app_id = args
        .app_id
        .parse::<u32>()
        .map_err(|_| "app ID must be a nonzero uint32")?;
    if app_id == 0 || app_id.to_string() != args.app_id {
        return Err("app ID must be a canonical nonzero uint32".into());
    }

    let challenge = read_bounded(&args.challenge, MAX_CHALLENGE_BYTES, "challenge file")?;
    let challenge_id = canonical_challenge(&challenge, &args.key_id, &args.publisher, app_id)?;
    output_destination(&args.output, "proof")?;
    // Only inspect the managed private key after every server-body context check has passed.
    let envelope = sign_managed(3, &args.key_id, args.directory, &challenge)?;
    let output_path = publish_public(&args.output, &envelope, "proof")?;
    let output_path = output_path.to_str().ok_or("proof output path is not valid UTF-8")?;
    serde_json::to_string(&serde_json::json!({"schema_version":1,"key_id":args.key_id,"challenge_id":challenge_id,"proof_path":output_path,"proof_sha256":hex(&Sha256::digest(&envelope))})).map_err(|_| "cannot encode proof result".into())
}
