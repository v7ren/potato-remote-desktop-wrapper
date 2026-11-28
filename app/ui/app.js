const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = (id) => document.getElementById(id);

// tiny DOM builder: h("div.cls", {attr}, ...children). Text goes in as text nodes, never HTML.
function h(tag, attrs, ...kids) {
  const [t, ...cls] = tag.split(".");
  const el = document.createElement(t);
  if (cls.length) el.className = cls.join(" ");
  for (const [k, v] of Object.entries(attrs || {})) {
    if (k.startsWith("on")) el.addEventListener(k.slice(2), v);
    else if (v === true) el.setAttribute(k, "");
    else if (v !== false && v != null) el.setAttribute(k, v);
  }
  for (const c of kids.flat()) if (c != null && c !== false) el.append(c);
  return el;
}
const icon = (id) => { const s = document.createElementNS("http://www.w3.org/2000/svg", "svg"); s.innerHTML = `<use href="#${id}"/>`; s.setAttribute("aria-hidden", "true"); return s; };

function toast(msg) {
  const t = h("div.toast", {}, String(msg));
  $("toasts").append(t);
  setTimeout(() => t.remove(), 4500);
}

let S = null;                 // last state
let hostUi = new Map();       // ip -> {screen, busy, error}: survives re-renders
let hostSig = "";
let signing = false;          // waiting for the browser sign-in
let pollTimer = null;

// ---------- state ----------
async function refresh() {
  try { S = await invoke("state"); } catch (e) { toast("Couldn't read state: " + e); return; }
  if (S.error) toast(S.error);
  const needs = S.missing && S.missing.length;
  const signedIn = S.signed_in && !needs;
  if (signedIn) signing = false;
  $("welcome").hidden = signedIn;
  $("app").hidden = !signedIn;
  if (!signedIn) {
    $("welcomeText").textContent = needs ? `Install ${S.missing.join(" and ")} first, then reopen Solinear.`
      : "Stream your PCs anywhere. Sign in once, then use the same account on every device.";
    $("signinBtn").hidden = !!needs;
    $("signinBtn").disabled = signing;
    $("signinBtn").textContent = signing ? "Finish in your browser…" : "Sign in";
    return;
  }
  $("acct").textContent = S.account || "Signed in";
  $("mod").textContent = S.mac ? "Ctrl+Option+Shift" : "Ctrl+Alt+Shift";
  renderHostRow();
  loadHosts();
}

function renderHostRow() {
  const kind = S.mac ? "Mac" : "PC";
  $("hostTitle").textContent = `Stream this ${kind}`;
  $("hostSub").textContent = S.is_host ? `This ${kind} is ready. Sign in on another device with the same account.` : `Let your other devices play on this ${kind}.`;
  $("hostRow").classList.toggle("on", !!S.is_host);
  const sw = $("hostSwitch");
  sw.setAttribute("aria-checked", String(!!S.is_host));
  sw.disabled = !S.can_host;
  const note = $("hostNote");
  note.hidden = false;
  if (!S.can_host) note.textContent = S.mac
    ? "To stream this Mac, install Sunshine (brew tap LizardByte/homebrew && brew install sunshine) and reopen Solinear."
    : "Streaming from this PC isn't available yet.";
  else if (S.is_host && S.mac) note.textContent = "If macOS asks for Screen Recording and Accessibility, allow both, then reopen Solinear.";
  else note.hidden = true;
}

$("hostSwitch").onclick = async (e) => {
  const sw = e.currentTarget, want = sw.getAttribute("aria-checked") !== "true";
  sw.disabled = true; sw.setAttribute("aria-checked", String(want));
  try { S = await invoke("set_hosting", { enabled: want }); }
  catch (err) { toast("Setup failed: " + err); }
  refresh();
};

// ---------- sign in / switch ----------
function waitForSignIn() {
  signing = true;
  refresh();
  clearInterval(pollTimer);
  pollTimer = setInterval(async () => { await refresh(); if (!signing) clearInterval(pollTimer); }, 1500);
}
$("signinBtn").onclick = async () => {
  $("signinBtn").disabled = true;
  try { await invoke("sign_in"); waitForSignIn(); }
  catch (e) { toast("Sign-in failed: " + e); $("signinBtn").disabled = false; }
};
$("switchBtn").onclick = async () => {
  try { await invoke("switch_account"); hostSig = ""; waitForSignIn(); }
  catch (e) { toast("Couldn't switch account: " + e); }
};

// ---------- hosts ----------
async function loadHosts() {
  let list;
  try { list = await invoke("hosts"); } catch (e) { return; }
  const sig = JSON.stringify(list);
  if (sig === hostSig) return;       // don't rebuild (and steal focus) when nothing changed
  hostSig = sig;
  const box = $("hosts");
  box.replaceChildren();
  if (!list.length) {
    box.className = "";
    box.append(h("div.empty", {}, icon("i-empty"),
      h("b", {}, "No PCs yet"),
      "Open Solinear on your gaming PC, sign in with the same account and turn on “Stream this PC”."));
    return;
  }
  box.className = "grid";
  for (const host of list) box.append(hostCard(host));
}

function hostCard(host) {
  const ui = hostUi.get(host.ip) || {};
  hostUi.set(host.ip, ui);
  const screens = host.screens || [];
  if (!ui.screen || !screens.some(s => s.id === ui.screen)) ui.screen = (screens.find(s => s.primary) || screens[0] || {}).id ?? null;

  const link = !host.online ? h("span", {}, "Offline")
    : [h("span.dot." + (host.direct ? "ok" : "warn")), host.direct ? "Direct" : "Relayed", host.ms != null ? ` · ${host.ms} ms` : null];
  const pick = screens.length > 1 && h("label.pick", {}, "Screen",
    h("select", { onchange: (e) => (ui.screen = e.target.value) },
      screens.map((s, i) => h("option", { value: s.id, selected: s.id === ui.screen }, `${s.name || "Screen " + (i + 1)} (${s.w}×${s.h})${s.primary ? " · main" : ""}`))));
  const err = h("div.err", { role: "alert", hidden: !ui.error }, icon("i-warn"), h("span", {}, ui.error || ""));
  const btn = h("button.btn.play", { disabled: !host.online || ui.busy }, "");
  const paint = () => {
    btn.disabled = !host.online || !!ui.busy;
    btn.replaceChildren(...(ui.busy ? [h("span.spin"), "Connecting…"] : [icon("i-play"), host.online ? "Play" : "Offline"]));
    err.hidden = !ui.error; err.lastChild.textContent = ui.error || "";
  };
  btn.onclick = async () => {
    ui.busy = true; ui.error = null; paint();
    try {
      const r = await invoke("play", { ip: host.ip, name: host.name, screen: screens.length > 1 ? ui.screen : null });
      if (!r.ok) ui.error = r.error || "Couldn't start the stream.";
    } catch (e) { ui.error = "Couldn't start the stream: " + e; }
    ui.busy = false; paint();
  };
  paint();
  return h("div.host" + (host.online ? "" : ".off"), {},
    h("div.top", {}, h("div.os", {}, icon(host.os === "mac" ? "i-mac" : "i-win"), h("i" + (host.online ? ".on" : ""), { title: host.online ? "Online" : "Offline" })),
      h("div", { style: "min-width:0" }, h("div.nm", { title: host.name }, host.name), h("div", {}, link))),
    pick, btn, err);
}

// ---------- settings sheet ----------
const sheet = $("sheet");
const focusables = () => [...sheet.querySelectorAll("button, select")].filter(e => !e.disabled);
function openSheet() {
  sheet.classList.add("open"); $("scrim").classList.add("open");
  sheet.removeAttribute("inert"); $("closeSheet").focus();
}
function closeSheet() {
  sheet.classList.remove("open"); $("scrim").classList.remove("open");
  sheet.setAttribute("inert", ""); $("settingsBtn").focus();
}
sheet.setAttribute("inert", "");
$("settingsBtn").onclick = openSheet;
$("closeSheet").onclick = closeSheet;
$("scrim").onclick = closeSheet;
document.addEventListener("keydown", (e) => {
  if (!sheet.classList.contains("open")) return;
  if (e.key === "Escape") closeSheet();
  if (e.key === "Tab") {   // keep focus inside the sheet
    const f = focusables(), first = f[0], last = f[f.length - 1];
    if (e.shiftKey && document.activeElement === first) { e.preventDefault(); last.focus(); }
    else if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first.focus(); }
  }
});

const readSettings = () => ({
  quality: $("quality").value, display_mode: $("display_mode").value,
  reverse_scroll: $("reverse_scroll").getAttribute("aria-checked") === "true",
  match_keys: $("match_keys").getAttribute("aria-checked") === "true",
});
async function loadSettings() {
  try {
    const s = await invoke("settings");
    $("quality").value = s.quality; $("display_mode").value = s.display_mode;
    $("reverse_scroll").setAttribute("aria-checked", String(!!s.reverse_scroll));
    $("match_keys").setAttribute("aria-checked", String(!!s.match_keys));
  } catch (e) { toast("Couldn't load settings: " + e); }
}
const save = async () => { try { await invoke("save_settings", readSettings()); } catch (e) { toast("Couldn't save settings: " + e); } };
$("quality").onchange = $("display_mode").onchange = save;
for (const id of ["reverse_scroll", "match_keys"]) $(id).onclick = (e) => {
  e.currentTarget.setAttribute("aria-checked", String(e.currentTarget.getAttribute("aria-checked") !== "true")); save();
};

// ---------- boot ----------
listen("toast", (e) => toast(e.payload));
listen("state-changed", () => { hostSig = ""; refresh(); });
loadSettings();
refresh();
setInterval(() => { if (!signing) refresh(); }, 15000);
