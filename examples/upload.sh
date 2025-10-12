#!/bin/bash
# Example: Upload a package to the repository

# Configuration
SERVER_URL="${SERVER_URL:-http://localhost:3000}"
PACKAGE_FILE="${1}"
REPO="${2:-sw1nn}"
ARCH="${3:-x86_64}"

if [ -z "$PACKAGE_FILE" ]; then
    echo "Usage: $0 <package-file.pkg.tar.zst> [repo] [arch]"
    echo ""
    echo "Examples:"
    echo "  $0 mypackage-1.0.0-1-x86_64.pkg.tar.zst"
    echo "  $0 mypackage-1.0.0-1-x86_64.pkg.tar.zst sw1nn x86_64"
    echo "  $0 mypackage-1.0.0-1-any.pkg.tar.zst testing any"
    exit 1
fi

if [ ! -f "$PACKAGE_FILE" ]; then
    echo "Error: Package file not found: $PACKAGE_FILE"
    exit 1
fi

echo "Uploading package to repository..."
echo "  File: $PACKAGE_FILE"
echo "  Repo: $REPO"
echo "  Arch: $ARCH"
echo "  Server: $SERVER_URL"
echo ""

# Upload the package
response=$(curl -s -w "\n%{http_code}" -X POST "$SERVER_URL/api/packages" \
    -F "file=@$PACKAGE_FILE" \
    -F "repo=$REPO" \
    -F "arch=$ARCH")

# Extract status code and body
http_code=$(echo "$response" | tail -n1)
body=$(echo "$response" | sed '$d')

if [ "$http_code" = "201" ]; then
    echo "✓ Package uploaded successfully!"
    echo ""
    echo "$body" | jq '.' 2>/dev/null || echo "$body"
else
    echo "✗ Upload failed with HTTP $http_code"
    echo ""
    echo "$body" | jq '.' 2>/dev/null || echo "$body"
    exit 1
fi
