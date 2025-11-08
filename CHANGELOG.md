# Changelog

- - -

## [Unreleased]

### Added
- Initial implementation of Arch package repository service
- REST API for package upload, listing, and deletion
- Pacman-compatible repository interface
- Custom pacman database generation (no `repo-add` dependency)
- OpenAPI documentation with RapiDoc UI at `/api-docs`
- File upload support in API documentation UI
- Comprehensive unit and integration tests
- Example scripts for package upload (bash and python)

### Features
- **Package Upload**: Multipart form upload with browser support in API docs
  - File picker for `.pkg.tar.zst` files
  - Optional `repo` and `arch` parameters
- **Package Listing**: Query packages with filters by name, repo, and architecture
- **Package Deletion**: Remove packages and auto-regenerate repository databases
- **Repository Database Generation**:
  - Generates `.db.tar.gz` and `.files.tar.gz` compatible with pacman
  - Works on any OS (not just Arch Linux)
  - Parses `.PKGINFO` from packages

### Technical Details
- **Language**: Rust
- **Web Framework**: axum 0.7
- **OpenAPI**: utoipa 5.x with utoipa-axum integration
- **Storage**: File-based with JSON metadata
- **Error Handling**: Custom error types using derive_more
- **Testing**: Unit tests + integration tests with 100% route coverage

### Fixed
- Route conflict on startup (consolidated file serving handlers)
- OpenAPI path double-nesting issue
- File upload schema in OpenAPI spec for browser compatibility

### Documentation
- README.md with usage examples
- TESTING.md with testing strategy and issue documentation
- CHANGELOG.md tracking changes
- Inline API documentation
