// Growth Layer Agent Helper -- exports ONE D-Bus method,
// org.growthlayer.AgentHelper.GetFocusedWindow, returning the focused
// window's WM_CLASS (app id) and owning PID. Nothing else: no window
// title, no keystrokes, no pointer position -- the same narrow contract
// the agent's other collectors (X11/Hyprland/Windows/macOS) already
// hold. See linux-collector's gnome_extension.rs for the Rust client
// that calls this.
//
// Exists ONLY because org.gnome.Shell.Eval has been gated behind
// "unsafe mode" (off by default) since GNOME 41 -- there is no other
// portal or public API that exposes the focused window's owning
// process on GNOME. See docs/02_ARCHITECTURE/
// AGENT_LINUX_CAPABILITY_MATRIX.md, which flagged this as the single
// largest open risk before this extension existed.
//
// Written against the GNOME 45+ ESM extension format (the only format
// GNOME 45+ loads) -- untested against a real GNOME Shell session (this
// project's dev environment has no GNOME Shell at all, see
// gnome_extension.rs's module doc comment); code-complete, not
// field-verified until run for real.

import Gio from 'gi://Gio';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

const IFACE_XML = `
<node>
  <interface name="org.growthlayer.AgentHelper">
    <method name="GetFocusedWindow">
      <arg type="s" direction="out" name="wm_class"/>
      <arg type="i" direction="out" name="pid"/>
    </method>
  </interface>
</node>`;

export default class GrowthLayerAgentHelperExtension extends Extension {
    enable() {
        this._dbusImpl = Gio.DBusExportedObject.wrapJSObject(IFACE_XML, this);
        this._dbusImpl.export(Gio.DBus.session, '/org/growthlayer/AgentHelper');
    }

    disable() {
        this._dbusImpl?.unexport();
        this._dbusImpl = null;
    }

    // D-Bus method implementation -- `wrapJSObject` reflects this by
    // name and maps its return array onto the interface's OUT args.
    // Empty wm_class ("" ) is this extension's own "no window focused"
    // signal (e.g. an empty workspace) -- the Rust client
    // (gnome_extension.rs) treats that as `Ok(None)`, not an error.
    GetFocusedWindow() {
        const win = global.display.focus_window;
        if (!win) return ['', 0];
        const wmClass = win.get_wm_class() ?? '';
        const pid = win.get_pid() ?? 0;
        return [wmClass, pid];
    }
}
