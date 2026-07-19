# Runtime dependencies are left to rpmbuild's built-in dependency
# generator (it scans the packaged binary's dynamic symbol table the
# same way `ldd`/`dpkg-shlibdeps` do and adds versioned Requires
# automatically) rather than hand-listing every shared library here —
# same reasoning as the .deb side's use of `dpkg-shlibdeps` in
# ../deb/build.sh: a hand-maintained list can silently drift from what
# the binary actually links against.
Name: growth-layer-agent
Version: %{_agent_version}
Release: 1%{?dist}
Summary: Growth Layer desktop agent
License: Proprietary
BuildArch: x86_64
%global _binary_payload w2.xzdio

%description
Lightweight per-user desktop agent that collects activity signals for
the Growth Layer product. Runs entirely in user space; never requires
root at runtime. See AG-LNX-003 in
CROSS_PLATFORM_LIGHTWEIGHT_CLIENT_AUTOPILOT.md.

%install
mkdir -p %{buildroot}/usr/bin
install -m 755 %{_agent_bin_path} %{buildroot}/usr/bin/growth-layer-agent

%files
/usr/bin/growth-layer-agent

%post
# Deliberately a no-op — see ../deb/postinst's doc comment for why:
# the agent registers its own per-user systemd --user autostart unit
# from a real user session, never from this root-run scriptlet, and an
# install/upgrade never needs to touch a running instance because
# Linux allows replacing an in-use executable file (unlike Windows).
exit 0

%preun
# Deliberately a no-op — see %post and ../deb/prerm's doc comment.
exit 0
