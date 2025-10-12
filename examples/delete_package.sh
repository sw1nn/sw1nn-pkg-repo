#!/bin/bash
# Example: Delete a package from the repository

SERVER_URL="${SERVER_URL:-http://localhost:3000}"
PACKAGE_NAME="${1}"
REPO="${2:-sw1nn}"
ARCH="${3:-x86_64}"

if [ -z "$PACKAGE_NAME" ]; then
    echo "Usage: $0 <package-name> [repo] [arch]"
    echo ""
    echo "Examples:"
    echo "  $0 mypackage"
    echo "  $0 mypackage sw1nn x86_64"
    echo "  $0 mypackage testing any"
    exit 1
fi

echo "Deleting package from repository..."
echo "  Package: $PACKAGE_NAME"
echo "  Repo: $REPO"
echo "  Arch: $ARCH"
echo "  Server: $SERVER_URL"
echo ""

# Delete the package
http_code=$(curl -s -w "%{http_code}" -o /dev/null -X DELETE \
    "$SERVER_URL/api/packages/$PACKAGE_NAME?repo=$REPO&arch=$ARCH")

if [ "$http_code" = "204" ]; then
    echo "✓ Package deleted successfully!"
elif [ "$http_code" = "404" ]; then
    echo "✗ Package not found"
    exit 1
else
    echo "✗ Delete failed with HTTP $http_code"
    exit 1
fi
