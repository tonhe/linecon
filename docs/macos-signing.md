# Signing macOS releases

Tagged releases are signed with a Developer ID certificate and notarized by Apple when these repository secrets
exist. Without them, builds are ad-hoc signed and Gatekeeper blocks them until allowed by hand. Pull requests and
ordinary pushes never receive these secrets.

| Secret | What it is |
|---|---|
| `MACOS_CERT_P12` | The Developer ID Application certificate and key, exported as .p12, then base64 encoded |
| `MACOS_CERT_PASSWORD` | The password chosen when exporting the .p12 |
| `APPLE_API_KEY_P8` | App Store Connect API key file, base64 encoded |
| `APPLE_API_KEY_ID` | That key's ID |
| `APPLE_API_ISSUER` | The issuer ID shown above the keys list |

## Setting up

1. In Xcode (Settings, Accounts, Manage Certificates) or the developer portal, create a **Developer ID Application**
   certificate. Not "Apple Development" or "Mac App Distribution".
2. In Keychain Access, find it under My Certificates, including its private key, and export it as a .p12.
3. In App Store Connect, go to Users and Access, Integrations, then **App Store Connect API** under Keys in the left
   column. The first time, the Account Holder has to click Request Access, and Team Keys appears once that is approved.
   Create a key with the **Developer** role and download the .p8. It can only be downloaded once.
4. Add the secrets:

       base64 -i cert.p12 | gh secret set MACOS_CERT_P12 -R tonhe/linecon
       gh secret set MACOS_CERT_PASSWORD -R tonhe/linecon
       base64 -i AuthKey_XXXX.p8 | gh secret set APPLE_API_KEY_P8 -R tonhe/linecon
       gh secret set APPLE_API_KEY_ID -R tonhe/linecon
       gh secret set APPLE_API_ISSUER -R tonhe/linecon

5. Delete the local .p12 and .p8, or keep them somewhere safe.

The next tag signs and notarizes by itself. If only some of the secrets are set, the release fails rather than
shipping unsigned by accident.
