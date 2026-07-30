// Growth Layer Agent Helper -- pushes ONE thing to the agent over D-Bus,
// org.growthlayer.AgentKWinBridge.SetFocusedWindow: the focused window's
// resource class (app id) and owning PID. Nothing else: no window title
// (KWin exposes `caption` right here and this script deliberately never
// reads it), no keystrokes, no pointer position -- the same narrow
// contract the agent's other collectors (X11/Hyprland/GNOME/Windows/
// macOS) already hold. See linux-collector's kwin_script.rs for the Rust
// side.
//
// The direction is INVERTED relative to the GNOME companion extension
// (../gnome-extension/extension.js), which exports a D-Bus method the
// agent calls. A KWin JS script cannot export anything: its entire API
// surface is callDBus/registerShortcut/readConfig/registerScreenEdge/
// registerUserActionsMenu (verified against libkwin.so.6's meta-object
// tables). So the script pushes and the agent owns the bus name.
//
// There is no timer available here either (no setTimeout, and QTimer is
// QML-only), which is why the load-time push below is load-bearing
// rather than a nicety: a script that has already been running since
// before the agent started can never be asked anything, so kwin_script.rs
// reloads it (unloadScript + start) to make it re-announce the current
// focus. Liveness is checked from the agent's side via isScriptLoaded.

const SERVICE = "org.growthlayer.AgentKWinBridge";
const PATH = "/org/growthlayer/AgentKWinBridge";
const IFACE = "org.growthlayer.AgentKWinBridge";

// Empty resource class ("") plus a zero PID is this script's own "no
// window focused" signal (e.g. an empty virtual desktop, or the lock
// screen taking focus) -- kwin_script.rs treats that as `Ok(None)`, not
// an error. Matches extension.js's empty-wm_class convention exactly.
function push(window) {
    if (!window) {
        callDBus(SERVICE, PATH, IFACE, "SetFocusedWindow", "", 0);
        return;
    }
    callDBus(SERVICE, PATH, IFACE, "SetFocusedWindow",
             String(window.resourceClass), window.pid);
}

workspace.windowActivated.connect(push);
push(workspace.activeWindow);
