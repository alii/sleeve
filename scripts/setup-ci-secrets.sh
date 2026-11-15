#!/bin/sh
# Gives GitHub what .github/workflows/release.yml needs to sign and notarize Sleeve: the
# Developer ID Application certificate and its key, and an App Store Connect API key. Run it
# once on the Mac that has the certificate, with gh logged in.
#
#   scripts/setup-ci-secrets.sh path/to/AuthKey_XXXXXXXXXX.p8 ISSUER_ID
#
# The key ID comes from the .p8's file name. macOS asks for your login password to export
# the certificate's key: click Allow.

set -eu
p8=${1:?usage: setup-ci-secrets.sh AuthKey_XXXXXXXXXX.p8 ISSUER_ID}
issuer=${2:?usage: setup-ci-secrets.sh AuthKey_XXXXXXXXXX.p8 ISSUER_ID}
key_id=$(basename "$p8" .p8 | sed 's/^AuthKey_//')
repo=${REPO:-alii/sleeve}

dir=$(mktemp -d)
kc="$dir/only-developer-id.keychain-db"
trap 'security delete-keychain "$kc" 2>/dev/null || true; rm -rf "$dir"' EXIT

echo "Exporting your signing certificates. Type your Mac password and click Allow if asked."
pass=$(openssl rand -hex 24)
security export -k "$HOME/Library/Keychains/login.keychain-db" -t identities -f pkcs12 -P "$pass" -o "$dir/all.p12"

# Into a throwaway keychain, so everything but the Developer ID one can be thrown out.
security create-keychain -p "$pass" "$kc"
security unlock-keychain -p "$pass" "$kc"
security import "$dir/all.p12" -k "$kc" -P "$pass" -A >/dev/null
security find-identity -v "$kc" | sed -n 's/^ *[0-9]*) \([0-9A-F]*\) "\(.*\)"$/\1 \2/p' | while read -r hash name; do
    case "$name" in
        "Developer ID Application:"*) echo "Keeping $name" ;;
        *) security delete-identity -Z "$hash" "$kc" >/dev/null ;;
    esac
done
[ "$(security find-identity -v -p codesigning "$kc" | grep -c 'Developer ID Application')" = 1 ] || {
    echo "Expected exactly one Developer ID Application certificate." >&2
    exit 1
}
cert_pass=$(openssl rand -hex 24)
security export -k "$kc" -t identities -f pkcs12 -P "$cert_pass" -o "$dir/developer-id.p12"

echo "Saving the secrets on ${repo}…"
base64 -i "$dir/developer-id.p12" | gh secret set MACOS_CERT_P12 -R "$repo"
printf '%s' "$cert_pass" | gh secret set MACOS_CERT_PASSWORD -R "$repo"
gh secret set ASC_KEY_P8 -R "$repo" < "$p8"
printf '%s' "$key_id" | gh secret set ASC_KEY_ID -R "$repo"
printf '%s' "$issuer" | gh secret set ASC_ISSUER_ID -R "$repo"
gh secret list -R "$repo"
echo "Done."
