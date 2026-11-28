// Loaded only when window.__TAURI__ is missing (opening the pages straight in a browser).
(() => {
  const listeners = {};
  let st = { signed_in: false, account: "ren@example.com", device_name: "Ren-MacBook", is_host: false, can_host: true, mac: false, missing: [], error: null };
  let settings = { quality: "auto", display_mode: "borderless", reverse_scroll: false, match_keys: true };
  const hosts = [
    { name: "Ren-pc", ip: "100.64.0.2", os: "windows", online: true, direct: true, ms: 12,
      screens: [{ id: "\\\\.\\DISPLAY1", name: "Display 1", w: 2560, h: 1440, primary: true }, { id: "\\\\.\\DISPLAY2", name: "Display 2", w: 1920, h: 1080, primary: false }] },
    { name: "Studio Mac", ip: "100.64.0.3", os: "mac", online: false, direct: false, ms: null, screens: [{ id: "1", name: "Built-in", w: 3024, h: 1964, primary: true }] },
  ];
  const q = new URLSearchParams(location.search);
  if (q.has("signedin")) st.signed_in = true;
  if (q.has("empty")) hosts.length = 0;
  const wait = (ms, v) => new Promise(r => setTimeout(() => r(v), ms));
  const cmds = {
    state: () => ({ ...st }),
    sign_in: async () => { setTimeout(() => { st.signed_in = true; emit("state-changed"); }, 2500); return { url: "https://login.tailscale.com/a/mock" }; },
    switch_account: async () => { st.signed_in = false; setTimeout(() => { st.signed_in = true; st.account = "other@example.com"; emit("state-changed"); }, 2500); return { url: "https://login.tailscale.com/a/mock2" }; },
    set_hosting: ({ enabled }) => { st.is_host = enabled; return { ...st }; },
    hosts: () => hosts,
    settings: () => settings,
    save_settings: (s) => (settings = s),
    play: async ({ name }) => { await wait(1500); return name === "Studio Mac" ? { ok: false, error: "Studio Mac isn't reachable." } : { ok: true, error: null }; },
    toolbar_info: () => ({ name: "Ren-pc", ip: "100.64.0.2", os: new URLSearchParams(location.search).get("os") || "windows", screens: hosts[0].screens, current_screen: "\\\\.\\DISPLAY1",
      opts: { quality: "auto", size: null, fps: null, hdr: false, display_mode: "borderless" } }),
  };
  function emit(name, payload) { (listeners[name] || []).forEach(f => f({ event: name, payload })); }
  window.__mockEmit = emit;
  window.__TAURI__ = {
    core: { invoke: async (c, a) => { console.log("invoke", c, a); await wait(40); return cmds[c] ? cmds[c](a) : null; } },
    event: { listen: async (n, f) => { (listeners[n] ||= []).push(f); return () => {}; } },
  };
})();
