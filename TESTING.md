## Testing Strategy

This document describes the testing strategy for the sw1nn-pkg-repo service and documents issues encountered during development.

### Issue: Server Crash on Startup

**Problem:** The server crashed on startup with a route conflict error:

```
Invalid route "/:repo/os/:arch/:db_file": insertion failed due to conflict with
previously registered route: /:repo/os/:arch/:filename
```

**Root Cause:** We had two separate route handlers both trying to match the same path pattern with different parameter names (`/:filename` and `/:db_file`). Axum's router couldn't distinguish between them since they're both catch-all parameters at the same level.

**Solution:** Consolidated into a single `serve_file()` handler that determines file type based on extension and serves both package files and database files from the appropriate location.

**Prevention:** Added integration tests that verify the server can start and routes are registered correctly.

### Test Structure

#### Unit Tests (`src/models/pkginfo.rs`)
Tests for `.PKGINFO` parsing logic:
- `test_parse_minimal_pkginfo` - Validates parsing with minimal required fields
- `test_parse_full_pkginfo` - Validates parsing with all optional fields
- `test_parse_missing_required_field` - Ensures proper error handling
- `test_parse_ignores_comments` - Validates comment handling

#### Integration Tests (`tests/integration_tests.rs`)
End-to-end tests for API endpoints:
- `test_server_starts_and_routes_registered` - Verifies server initialization and route registration
- `test_list_packages_empty` - Tests empty package list response
- `test_serve_file_not_found` - Tests 404 handling for missing files
- `test_delete_package_not_found` - Tests 404 handling for missing packages

### Running Tests

```bash
# Run all tests
cargo test

# Run specific test
cargo test test_name

# Run with output
cargo test -- --nocapture

# Run only unit tests
cargo test --lib

# Run only integration tests
cargo test --test integration_tests
```

### Test Coverage Areas

1. **Route Registration** - Ensures no route conflicts
2. **API Endpoints** - Basic CRUD operations
3. **Error Handling** - 404s, invalid inputs
4. **Package Parsing** - .PKGINFO extraction and validation
5. **File Serving** - Both packages and database files

### Future Test Improvements

1. **Package Upload Tests** - Full end-to-end package upload with mock .pkg.tar.zst files
2. **Database Generation Tests** - Verify .db and .files tar.gz creation
3. **Checksum Validation** - Test SHA256 and MD5 calculation
4. **Concurrent Operations** - Test thread safety and concurrent uploads
5. **Storage Tests** - File system operations and cleanup
6. **Configuration Tests** - Environment variable and file-based config loading

### CI/CD Considerations

- Tests should be run on every commit
- Consider adding cargo-tarpaulin for code coverage
- Add clippy and rustfmt checks
- Test on multiple platforms (Linux, macOS, Windows)
