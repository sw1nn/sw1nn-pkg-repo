#!/usr/bin/env python3
"""
Example: Upload a package to the repository using Python
"""

import sys
import os
import requests
from pathlib import Path

def upload_package(package_file: str, repo: str = "sw1nn", arch: str = "x86_64",
                   server_url: str = "http://localhost:3000"):
    """Upload a package to the repository"""

    package_path = Path(package_file)

    if not package_path.exists():
        print(f"Error: Package file not found: {package_file}")
        return False

    print("Uploading package to repository...")
    print(f"  File: {package_file}")
    print(f"  Repo: {repo}")
    print(f"  Arch: {arch}")
    print(f"  Server: {server_url}")
    print()

    # Prepare the multipart form data
    with open(package_path, 'rb') as f:
        files = {
            'file': (package_path.name, f, 'application/zstd')
        }
        data = {
            'repo': repo,
            'arch': arch
        }

        # Upload the package
        try:
            response = requests.post(
                f"{server_url}/api/packages",
                files=files,
                data=data
            )

            if response.status_code == 201:
                print("✓ Package uploaded successfully!")
                print()
                print(response.json())
                return True
            else:
                print(f"✗ Upload failed with HTTP {response.status_code}")
                print()
                print(response.text)
                return False

        except requests.exceptions.RequestException as e:
            print(f"✗ Upload failed: {e}")
            return False

def main():
    if len(sys.argv) < 2:
        print("Usage: upload.py <package-file.pkg.tar.zst> [repo] [arch]")
        print()
        print("Examples:")
        print("  upload.py mypackage-1.0.0-1-x86_64.pkg.tar.zst")
        print("  upload.py mypackage-1.0.0-1-x86_64.pkg.tar.zst sw1nn x86_64")
        print("  upload.py mypackage-1.0.0-1-any.pkg.tar.zst testing any")
        sys.exit(1)

    package_file = sys.argv[1]
    repo = sys.argv[2] if len(sys.argv) > 2 else "sw1nn"
    arch = sys.argv[3] if len(sys.argv) > 3 else "x86_64"
    server_url = os.environ.get("SERVER_URL", "http://localhost:3000")

    success = upload_package(package_file, repo, arch, server_url)
    sys.exit(0 if success else 1)

if __name__ == "__main__":
    main()
