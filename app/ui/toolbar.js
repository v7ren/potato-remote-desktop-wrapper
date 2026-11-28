// the toolbar runs in its own process: every command goes through the overlay's `fwd` to the main app
const invoke = (cmd, args) => window.__TAURI__.core.invoke("fwd", { name: cmd, args: args || {} });
const { listen } = window.__TAURI__.event;
const $ = (id) => document.getElementById(id);
const MAC = /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);
const MOD = MAC ? "Ctrl+Option+Shift" : "Ctrl+Alt+Shift";

let info = { name: "", os: "windows", screens: [], current_screen: null, opts: {} };
let opts = {};
let stack = [];      // menu views; the last one is showing
let gaming = false;

const ico = (id, cls) => { const s = document.createElementNS("http://www.w3.org/2000/svg", "svg"); s.innerHTML = `<use href="#${id}"/>`; if (cls) s.setAttribute("class", cls); s.setAttribute("aria-hidden", "true"); return s; };
function el(tag, cls, ...kids) { const e = document.createElement(tag); if (cls) e.className = cls; e.append(...kids.filter(k => k != null)); return e; }
const call = (cmd, args) => invoke(cmd, args).catch((e) => console.error(cmd, e));

// ---- window size: tell the backend how big the content is whenever it changes ----
function fit() {
  requestAnimationFrame(() => {
    const r = $("wrap").getBoundingClientRect();
    call("toolbar_resize", { w: Math.ceil(r.width) + 4, h: Math.ceil(r.height) + 4 });
  });
}

// ---- menu model ----
const reconnect = (patch) => { Object.assign(opts, patch); call("toolbar_reconnect", patch); closeMenu(); };
const choice = (title, key, options) => ({
  title, items: options.map(([label, value]) => ({ label, check: opts[key] === value || (value == null && opts[key] == null), run: () => reconnect({ [key]: value }) })),
});
const macHost = () => info.os === "mac";
const act = (name, label) => ({ label, run: () => { call("toolbar_action", { name }); closeMenu(); } });
const confirmAct = (name, label) => ({ label, run: () => push({ confirm: `${label} ${info.name}?`, yes: label, run: () => { call("toolbar_action", { name }); closeMenu(); } }) });

const views = {
  root: () => ({ items: [
    { label: "Desktop", go: views.desktop },
    { label: "Keyboard and shortcuts", go: views.keyboard },
    "sep",
    { label: "Stream stats", hint: `${MOD}+S`, run: () => { call("toolbar_combo", { key: "s" }); closeMenu(); } },
    "sep",
    { label: "End session", hint: `${MOD}+Q`, run: () => { call("toolbar_end"); closeMenu(); } },
  ] }),
  desktop: () => ({ title: "Desktop", items: [
    ...(info.screens || []).slice(0, 12).map((s, i) => ({ label: `Screen ${i + 1}` + (s.w ? `  (${s.w}×${s.h})` : ""), check: s.id === info.current_screen,
      run: () => { switchScreen(s.id); closeMenu(); } })),
    "sep", { note: "Changes below reconnect the stream" },
    { label: "Quality", go: () => choice("Quality", "quality", [["Smoother", "smoother"], ["Auto", "auto"], ["Sharper", "sharper"]]) },
    { label: "Resolution", go: () => choice("Resolution", "size", [["Match this screen", null], ["1280×720", "1280x720"], ["1920×1080", "1920x1080"], ["2560×1440", "2560x1440"], ["3840×2160", "3840x2160"]]) },
    { label: "Frame rate", go: () => choice("Frame rate", "fps", [["Match this screen", null], ["30 fps", 30], ["60 fps", 60], ["120 fps", 120], ["144 fps", 144]]) },
    { label: "Window", go: () => choice("Window", "display_mode", [["Borderless fullscreen", "borderless"], ["Windowed", "windowed"], ["Fullscreen (hides this toolbar)", "fullscreen"]]) },
    { label: "HDR", check: !!opts.hdr, run: () => reconnect({ hdr: !opts.hdr }) },
  ] }),
  keyboard: () => ({ title: "Keyboard and shortcuts", items: [
    act("lock", "Lock"), confirmAct("signout", "Sign out"), confirmAct("restart", "Restart"), confirmAct("shutdown", "Shut down"),
    "sep",
    act("explorer", macHost() ? "Finder" : "File Explorer"),
    act("taskmgr", macHost() ? "Activity Monitor" : "Task Manager"),
    act("control", macHost() ? "System Settings" : "Control Panel"),
  ] }),
};

// ---- menu rendering ----
function push(v) { stack.push(typeof v === "function" ? v() : v); render(); }
function pop() { stack.pop(); stack.length ? render() : closeMenu(); }
const MENU_ONLY = !!window.__SOLINEAR_MENU;  // opened from the in-stream toolbar's MORE: no pill, gone with the menu
function closeMenu() { if (MENU_ONLY) return call("toolbar_quit"); stack = []; $("panel").hidden = true; $("menuBtn").setAttribute("aria-expanded", "false"); fit(); $("menuBtn").focus(); }
function openMenu() { stack = [views.root()]; $("menuBtn").setAttribute("aria-expanded", "true"); render(); }

function render() {
  const v = stack[stack.length - 1], p = $("panel");
  p.hidden = false; p.replaceChildren();
  if (v.confirm) {
    p.append(el("div", "ask", v.confirm));
    const no = el("button", "", "Cancel"), yes = el("button", "danger", v.yes);
    no.onclick = pop; yes.onclick = v.run;
    p.append(el("div", "btns", no, yes));
    no.focus();
    return fit();
  }
  if (v.title) {
    const b = el("button", "it back", ico("back"), el("span", "lab", v.title));
    b.setAttribute("role", "menuitem"); b.onclick = pop; p.append(b);
    p.append(el("div", "sep"));
  }
  for (const it of v.items) {
    if (it === "sep") { p.append(el("div", "sep")); continue; }
    if (it.note) { p.append(el("div", "note", it.note)); continue; }
    const b = el("button", "it", el("span", "lab", it.label), it.hint ? el("span", "hint", it.hint) : null,
      it.check ? ico("check", "ck") : null, it.go ? ico("chev", "ck") : null);
    b.setAttribute("role", it.check != null ? "menuitemradio" : "menuitem");
    if (it.check != null) b.setAttribute("aria-checked", String(!!it.check));
    b.onclick = it.go ? () => push(it.go) : it.run;
    p.append(b);
  }
  p.querySelector(".it:not(.back)")?.focus();
  fit();
}

$("panel").addEventListener("keydown", (e) => {
  const items = [...$("panel").querySelectorAll("button")], i = items.indexOf(document.activeElement);
  if (e.key === "ArrowDown") { items[(i + 1) % items.length]?.focus(); e.preventDefault(); }
  else if (e.key === "ArrowUp") { items[(i - 1 + items.length) % items.length]?.focus(); e.preventDefault(); }
  else if (e.key === "Escape" || e.key === "ArrowLeft") { pop(); e.preventDefault(); }
});
$("menuBtn").onclick = () => ($("panel").hidden ? openMenu() : closeMenu());

// ---- pill ----
$("fullBtn").title = $("fullBtn").ariaLabel = `Fullscreen / window (${MOD}+X)`;
$("gameBtn").title = `Gaming mode captures the mouse. ${MOD}+M toggles it.`;
$("fullBtn").onclick = () => call("toolbar_combo", { key: "x" });
const setGaming = (on) => { gaming = on; $("gameBtn").setAttribute("aria-pressed", String(on)); };
$("gameBtn").onclick = () => { setGaming(!gaming); call("toolbar_combo", { key: "m" }); };
listen("gaming-mode", (e) => setGaming(!!e.payload));

// Screen 1 | 2 on the pill (only when the host has more than one)
async function switchScreen(id) {
  const was = info.current_screen;
  info.current_screen = id; paintScreens();
  const r = await call("toolbar_switch_screen", { id });   // undefined if it failed
  info.current_screen = r ? r.current_screen : was;          // light the screen that's really showing
  paintScreens();
}
function paintScreens() {
  const box = $("screens"), list = (info.screens || []).slice(0, 12);
  box.hidden = list.length < 2;
  box.replaceChildren(...list.map((s, i) => {
    const b = el("button", "", String(i + 1));
    b.title = `Screen ${i + 1}` + (s.w ? ` (${s.w}×${s.h})` : "") + (i < 12 ? ` · ${MOD}+F${i + 1}` : "");
    b.setAttribute("aria-pressed", String(s.id === info.current_screen));
    b.onclick = () => switchScreen(s.id);
    return b;
  }));
  fit();
}

// remember where the pill was dropped
$("grip").addEventListener("pointerup", () => call("toolbar_drag_end", { x: window.screenX, y: window.screenY }));
$("pill").addEventListener("pointerup", (e) => { if (e.target === $("pill")) call("toolbar_drag_end", { x: window.screenX, y: window.screenY }); });

(async () => {
  try { info = await invoke("toolbar_info"); } catch (e) { console.error(e); }
  opts = { ...(info.opts || {}) };
  paintScreens();
  if (MENU_ONLY) { $("pill").hidden = true; openMenu(); }
})();
