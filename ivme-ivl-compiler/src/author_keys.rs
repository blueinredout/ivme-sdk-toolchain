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

pub fn dispatch() -> ! {
    let parsed = match AuthorKeyCli::try_parse_from(env::args_os()) {
        Ok(cli) => cli,
        Err(error) => {
            let code = error.exit_code();
            let _ = error.print();
            process::exit(code);
        }
    };
    let NamespaceCommand::AuthorKey(args) = parsed.command;
    let result = match args.command {
        AuthorKeyCommand::Create(args) => create(args.directory),
        AuthorKeyCommand::Info(args) => info(&args.key_id, args.directory),
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

fn info(key_id: &str, directory: Option<PathBuf>) -> Result<String, String> {
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
    let dir = dir.to_str().ok_or("key directory is not valid UTF-8")?;
    serde_json::to_string(&serde_json::json!({"schema_version": 1, "key_id": actual, "public_key_pem": pem, "key_directory": dir})).map_err(|e| e.to_string())
}
