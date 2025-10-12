#!/bin/bash
# Example: List packages in the repository

SERVER_URL="${SERVER_URL:-http://localhost:3000}"

echo "Listing packages from repository..."
echo "  Server: $SERVER_URL"
echo ""

# Parse command line arguments
NAME=""
REPO=""
ARCH=""

while [[ $# -gt 0 ]]; do
    case $1 in
        --name)
            NAME="$2"
            shift 2
            ;;
        --repo)
            REPO="$2"
            shift 2
            ;;
        --arch)
            ARCH="$2"
            shift 2
            ;;
        --help)
            echo "Usage: $0 [--name <package-name>] [--repo <repo>] [--arch <arch>]"
            echo ""
            echo "Examples:"
            echo "  $0                                  # List all packages"
            echo "  $0 --name mypackage                 # Filter by package name"
            echo "  $0 --repo custom --arch x86_64      # Filter by repo and arch"
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            echo "Use --help for usage information"
            exit 1
            ;;
    esac
done

# Build query string
QUERY=""
if [ -n "$NAME" ]; then
    QUERY="${QUERY}name=${NAME}&"
fi
if [ -n "$REPO" ]; then
    QUERY="${QUERY}repo=${REPO}&"
fi
if [ -n "$ARCH" ]; then
    QUERY="${QUERY}arch=${ARCH}&"
fi

# Remove trailing &
QUERY="${QUERY%&}"

# Build URL
URL="$SERVER_URL/api/packages"
if [ -n "$QUERY" ]; then
    URL="${URL}?${QUERY}"
fi

# Fetch packages
response=$(curl -s "$URL")

if [ $? -eq 0 ]; then
    echo "$response" | jq '.' 2>/dev/null || echo "$response"
else
    echo "✗ Failed to fetch packages"
    exit 1
fi
