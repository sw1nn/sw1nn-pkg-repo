# Arch Linux Package Repository Service

A self-hosted Arch Linux package repository service written in Rust. This service allows you to upload and manage custom Arch packages and provides both a REST API and a pacman-compatible repository interface.

## Features

- **REST API** for package management (upload, list, delete)
- **OpenAPI documentation** with interactive RapiDoc UI
- **Pacman-compatible repository** interface
- **Custom database generation** - no dependency on `repo-add` tools
- **File-based storage** - simple and fast for small package sets
- **Automatic metadata generation** - creates `.db` and `.files` databases

## Architecture

The service parses `.pkg.tar.zst` files to extract `.PKGINFO` metadata and generates pacman repository databases (`.db.tar.gz` and `.files.tar.gz`) without requiring Arch Linux tools. This allows it to run on any operating system.

## Configuration

The service loads configuration from multiple sources in order of precedence (highest to lowest):

1. **Environment variables** (highest priority)
2. **Command-line specified config file** (`--config`)
3. **Default config location** (depends on build type):
   - **Release builds**: `/etc/sw1nn-pkg-repo/config.toml`
   - **Debug builds**: `./config.toml` (current working directory)
4. **Built-in defaults** (lowest priority)

### Configuration File

Create a `config.toml` file in one of the locations above:

```toml
[server]
host = "0.0.0.0"
port = 3000

[storage]
data_path = "./data"
default_repo = "sw1nn"
default_arch = "x86_64"
```

### Environment Variables

```bash
export PKG_REPO_SERVER__HOST="0.0.0.0"
export PKG_REPO_SERVER__PORT="3000"
export PKG_REPO_STORAGE__DATA_PATH="./data"
export PKG_REPO_STORAGE__DEFAULT_REPO="sw1nn"
export PKG_REPO_STORAGE__DEFAULT_ARCH="x86_64"
```

### Command-line Options

Specify a custom configuration file:

```bash
sw1nn-pkg-repod --config /path/to/custom-config.toml
```

## Authentication

The API endpoints accept OIDC access tokens issued by
[Authelia](https://www.authelia.com/). The server verifies them offline against
a cached JWKS — it never calls the introspection or userinfo endpoints, so the
identity provider is not in the request path for uploads.

Add an `[auth]` section to `config.toml` to turn this on. Every field has a
default, so the section can be empty:

```toml
[auth]
issuer = "https://auth.sw1nn.net"
jwks_uri = "https://auth.sw1nn.net/jwks.json"
client_id = "sw1nn-pkg-cli"
leeway_secs = 60
```

> [!WARNING]
> Without an `[auth]` section, every endpoint is publicly accessible.

A token is accepted only when its signature verifies, its `iss` matches, and
its `client_id` is this service's own. The `client_id` check is what stops a
token minted for another CLI being replayed here — Authelia's device grant
returns an empty `aud`, so an audience check is not available.

### Authorization

Access is granted by Authelia group membership. Scopes are not an authorization
boundary: the CLI is a public client, so a caller can request any scope it
likes.

| Operation | Required group |
| --- | --- |
| List packages | any valid token |
| Upload | `pkg-publish` |
| Delete, rebuild database, apply cleanup policy | `pkg-admin` |

Membership of `admins` grants nothing on its own.

The pacman-facing routes (`/{repo}/os/{arch}/{filename}`) stay unauthenticated,
so installing packages needs no credentials.

### Logging in

```bash
sw1nn-pkg-ctl login     # device flow: approve in a browser
sw1nn-pkg-ctl status    # who you are and when the token expires
sw1nn-pkg-ctl logout    # discard stored credentials
```

`login` prints a URL and a user code, and opens the URL if it can. Tokens are
kept in the Secret Service (KeepassXC on these machines), falling back to a
`0600` file under `$XDG_STATE_HOME/sw1nn-pkg-repo/` on headless hosts.

The access token lasts an hour and is renewed automatically from the refresh
token, so routine use needs one browser trip a month. When renewal fails the
command exits non-zero and tells you to log in again.

Point the CLI at a different deployment with `SW1NN_AUTH_ISSUER` and
`SW1NN_AUTH_CLIENT_ID`, and at a different repository with `SW1NN_REPO_URL`.

## Running

```bash
cargo run --release
```

The server will start on `http://127.0.0.1:3000` by default.

## API Documentation

Access the interactive API documentation at: `http://127.0.0.1:3000/api-docs`

The documentation UI supports file uploads directly from the browser for the package upload endpoint. You can:
1. Click on the `POST /api/packages` endpoint
2. Click "Try it out"
3. Use the file picker to select a `.pkg.tar.zst` file
4. Optionally specify `repo` and `arch` parameters
5. Click "Execute" to upload

Alternatively, access the OpenAPI spec directly at: `http://127.0.0.1:3000/api-docs/openapi.json`

## REST API Endpoints

### Upload Package

```bash
curl -X POST http://localhost:3000/api/packages \
  -F "file=@my-package-1.0.0-1-x86_64.pkg.tar.zst" \
  -F "repo=custom" \
  -F "arch=x86_64"
```

### List Packages

```bash
# List all packages
curl http://localhost:3000/api/packages

# Filter by name
curl http://localhost:3000/api/packages?name=my-package

# Filter by repo and arch
curl http://localhost:3000/api/packages?repo=custom&arch=x86_64
```

### Delete Package

```bash
curl -X DELETE http://localhost:3000/api/packages/my-package?repo=custom&arch=x86_64
```

## Using with Pacman

Add the repository to your `/etc/pacman.conf`:

```ini
[sw1nn]
SigLevel = Required DatabaseOptional TrustedOnly
Server = http://localhost:3000/$repo/os/$arch
```

`Required` rejects any unsigned package; `TrustedOnly` requires the
signing key to be in your local pacman keyring; `DatabaseOptional`
allows the repo database itself to be unsigned (the server does not
sign the `.db`/`.files` databases).

Import and locally sign the packaging key before first use, otherwise
pacman will reject every package:

```bash
sudo pacman-key --recv-keys <KEYID>
sudo pacman-key --lsign-key <KEYID>
```

Then update and install packages:

```bash
sudo pacman -Sy
sudo pacman -S my-package
```

## Project Structure

```
src/
├── bin/
│   ├── sw1nn-pkg-repod.rs      # Service binary
│   └── sw1nn-pkg-ctl/          # Client binary
│       ├── main.rs             #   commands and output
│       ├── authelia.rs         #   OIDC device flow (RFC 8628)
│       ├── client.rs           #   HTTP client with token renewal
│       └── token_store.rs      #   Secret Service / file credential storage
├── auth.rs                     # Access-token verification and group checks
├── lib.rs                      # Library code
└── ...

packaging/                      # Distribution packaging files (not build output)
└── arch/                       # Arch Linux PKGBUILD and related files

assets/                         # Deployment assets and configuration files
├── config.toml                 # Service configuration
└── sw1nn-pkg-repo.service      # systemd service file

data/                           # Runtime data directory (repository storage)
├── sw1nn/                      # Repository name
│   └── os/
│       └── x86_64/             # Architecture
│           ├── *.pkg.tar.zst        # Package files
│           ├── sw1nn.db             # Repository database (symlink)
│           ├── sw1nn.db.tar.gz      # Repository database
│           ├── sw1nn.files          # Files database (symlink)
│           ├── sw1nn.files.tar.gz   # Files database
│           └── metadata/
│               └── *.json           # Package metadata
```

**Note:** The `packaging/` directory contains distribution-specific packaging metadata (like PKGBUILD for Arch Linux), not compiled binaries. Cargo build outputs go to the `target/` directory as usual.

## Development

```bash
# Check code
cargo check

# Run tests
cargo test

# Run with debug logging
RUST_LOG=debug cargo run
```

## Dependencies

- **axum** - Web framework
- **utoipa** - OpenAPI generation
- **utoipa-rapidoc** - API documentation UI
- **tar** - TAR archive handling
- **flate2** - gzip compression
- **zstd** - zstd decompression for packages
- **tokio** - Async runtime
- **jsonwebtoken** - Access-token verification against Authelia's JWKS
- **keyring** - Secret Service credential storage for the client

## License

MIT
