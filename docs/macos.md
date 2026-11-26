# macOS builds

The macOS bundle contains the pinned stock Mihomo 1.19.34 build for the host architecture and the existing Clashfork launchd service tools. Move `Clashfork.app` into `/Applications` before connecting; the protected service only accepts cores from that trusted app location or its own root-owned install directory. The first Connect action asks for administrator approval to install or repair the protected service and core. Clashfork itself continues to run as the signed-in user; credentials are stored in that user's macOS Keychain.

The app configures Mihomo's TUN device as `utun1000` and checks the kernel's best route for two public addresses against that exact interface. Connection status requires the authenticated Mihomo controller's requested mode and TUN state plus successful route ownership checks, unless the route API is indeterminate. Disconnect retains the core only when controller and route checks confirm TUN is off; otherwise it stops the owned service session.

To remove the privileged service and its installed core, run `Uninstall Clashfork Service.command` inside `Clashfork.app/Contents/Resources/resources`. macOS asks for administrator approval. After it finishes, move the app to the Trash.

GitHub CI builds separate Apple silicon and Intel DMGs. They are unsigned preview artifacts until Developer ID signing and notarization credentials are configured. On disposable hosted runners, CI verifies the packaged app and core, launches the GUI, installs the service with a pinned core digest, exercises authenticated Rule and Global TUN lifecycle checks, and removes the service. This does not replace testing with a signed build on a supported end-user Mac before public release.
