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

Create a `config.toml` file (optional):

```toml
[server]
host = "0.0.0.0"
port = 3000

[storage]
data_path = "./data"
default_repo = "sw1nn"
default_arch = "x86_64"
```

Or use environment variables:

```bash
export PKG_REPO_SERVER__HOST="0.0.0.0"
export PKG_REPO_SERVER__PORT="3000"
export PKG_REPO_STORAGE__DATA_PATH="./data"
export PKG_REPO_STORAGE__DEFAULT_REPO="sw1nn"
export PKG_REPO_STORAGE__DEFAULT_ARCH="x86_64"
```

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
SigLevel = Optional TrustAll
Server = http://localhost:3000/$repo/os/$arch
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
│   ├── sw1nn-pkg-repo.rs      # Service binary
│   └── sw1nn-pkg-upload.rs    # Upload client binary
├── lib.rs                      # Library code
└── ...

dist/                           # Distribution packaging files (not build output)
└── arch/                       # Arch Linux PKGBUILD and related files

etc/                            # Example configuration files
├── config.toml                 # Service configuration
├── sw1nn-pkg-repo.service      # systemd service file
└── nginx-repo.sw1nn.net.conf   # nginx reverse proxy config

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

**Note:** The `dist/` directory contains distribution-specific packaging metadata (like PKGBUILD for Arch Linux), not compiled binaries. Cargo build outputs go to the `target/` directory as usual.

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

## License

MIT
