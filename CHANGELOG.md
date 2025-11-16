# Changelog

- - -
## v1.4.0 - 2025-11-16
#### Features
- (**logging**) add systemd-journald support for server - (ad9f5cc) - Neale Swinnerton

- - -

## v1.3.0 - 2025-11-12
#### Bug Fixes
- (**pkgbuild**) update source URL to use GitHub - (4e692bf) - Neale Swinnerton
#### Miscellaneous Chores
- (**build**) replace cargo set-version with sed approach - (ee45f4a) - Neale Swinnerton

- - -

## v1.2.0 - 2025-11-12
#### Features
- (**error**) add path logging for all I/O errors - (8f02691) - Neale Swinnerton
#### Refactoring
- (**error**) simplify map_err calls to use map_io_err - (fb9adef) - Neale Swinnerton

- - -

## v1.1.0 - 2025-11-10
#### Miscellaneous Chores
- (**version**) v1.0.2 - (e2ce8cb) - Neale Swinnerton

- - -

## v1.0.2 - 2025-11-10
#### Bug Fixes
- make sure that cog releases upload all the packages - (830eacb) - Neale Swinnerton

- - -

## v1.0.1 - 2025-11-10
#### Bug Fixes
- (**upload**) cap chunk size to file size for small packages - (025acd2) - Neale Swinnerton

- - -

## v1.0.0 - 2025-11-10
#### Features
- <span style="background-color: #d73a49; color: white; padding: 2px 6px; border-radius: 3px; font-weight: bold; font-size: 0.85em;">BREAKING</span>implement chunked upload with streaming assembly and remove legacy API - (16e8d52) - Neale Swinnerton

- - -

## v0.11.0 - 2025-11-08
#### Miscellaneous Chores
- (**cog**) tweak post_bump_hooks - (825cf4e) - Neale Swinnerton

- - -

## v0.10.0 - 2025-11-08
#### Features
- support multiple file uploads in CLI - (78ad573) - Neale Swinnerton
#### Miscellaneous Chores
- ignore package signature files - (76c3f90) - Neale Swinnerton

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
