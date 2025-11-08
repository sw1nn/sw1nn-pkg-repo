# Changelog

- - -
## v0.9.0 - 2025-11-08
#### Miscellaneous Chores
- build the arch package after a release - (86b2239) - Neale Swinnerton

- - -

## v0.8.0 - 2025-11-08
#### Features
- list all packages when no query parameters provided - (f2a642c) - Neale Swinnerton
- normalize data_path to absolute path on config load - (bd0ab22) - Neale Swinnerton
- align colored output field values - (b21e805) - Neale Swinnerton
- add colored, structured output to upload client - (8f5322a) - Neale Swinnerton
#### Bug Fixes
- correct path normalization to preserve absolute paths - (52da4da) - Neale Swinnerton
#### Miscellaneous Chores
- add changelog separator for cocogitto - (be916d1) - Neale Swinnerton
- migrate to cocogitto and standardize PKGBUILD - (c7f8898) - Neale Swinnerton

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
