// solinear-net: an embedded Tailscale node (tsnet) with a small local control API.
// See "Network helper" in ../CONTRACT.md.
package main

import (
	"bytes"
	"context"
	"crypto/subtle"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"log"
	"net"
	"net/http"
	"net/http/httputil"
	"net/netip"
	"net/url"
	"os"
	"sort"
	"strconv"
	"strings"
	"sync"
	"time"

	"tailscale.com/client/local"
	"tailscale.com/tailcfg"
	"tailscale.com/tsnet"
)

// Same ports on both ends: Moonlight takes them from what the host announces.
var (
	tcpPorts  = []int{57984, 57989, 57990, 58010}
	udpPorts  = []int{57998, 57999, 58000, 58002}
	agentPort = 57980
)

type (
	dialFunc  func(ctx context.Context, network, addr string) (net.Conn, error)
	allowFunc func(remote net.Addr) bool
)

// ---- forwarding (plain net code, tested on loopback) ----

func forwardTCP(ln net.Listener, dial dialFunc, target string, allow allowFunc) {
	for {
		c, err := ln.Accept()
		if err != nil {
			return
		}
		if allow != nil && !allow(c.RemoteAddr()) {
			c.Close()
			continue
		}
		go func() {
			ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
			up, err := dial(ctx, "tcp", target)
			cancel()
			if err != nil {
				c.Close()
				return
			}
			done := make(chan struct{}, 2)
			pipe := func(dst, src net.Conn) { io.Copy(dst, src); done <- struct{}{} }
			go pipe(up, c)
			go pipe(c, up)
			<-done // either side closing ends the session
			c.Close()
			up.Close()
		}()
	}
}

// forwardUDP keeps one dialed socket per remote ip:port; replies go back to that remote.
// A session dies after `idle` without a reply. Returns when pc is closed.
func forwardUDP(pc net.PacketConn, dial dialFunc, target string, allow allowFunc, idle time.Duration) {
	var mu sync.Mutex
	sessions := map[string]net.Conn{}
	defer func() {
		mu.Lock()
		for _, c := range sessions {
			c.Close()
		}
		mu.Unlock()
	}()
	buf := make([]byte, 65535)
	for {
		n, remote, err := pc.ReadFrom(buf)
		if err != nil {
			return
		}
		if allow != nil && !allow(remote) {
			continue
		}
		key := remote.String()
		mu.Lock()
		c := sessions[key]
		if c == nil {
			ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
			c, err = dial(ctx, "udp", target)
			cancel()
			if err != nil {
				mu.Unlock()
				continue
			}
			sessions[key] = c
			go func() {
				defer func() { mu.Lock(); delete(sessions, key); mu.Unlock(); c.Close() }()
				rb := make([]byte, 65535)
				for {
					c.SetReadDeadline(time.Now().Add(idle))
					m, err := c.Read(rb)
					if err != nil {
						return
					}
					pc.WriteTo(rb[:m], remote)
				}
			}()
		}
		mu.Unlock()
		c.Write(buf[:n])
	}
}

// ---- agent reverse proxy ----

// agentProxy forwards to target, replacing any caller-supplied X-Solinear-* headers.
func agentProxy(target *url.URL, secret string, login func(*http.Request) string) http.Handler {
	return &httputil.ReverseProxy{Rewrite: func(pr *httputil.ProxyRequest) {
		pr.SetURL(target)
		h := pr.Out.Header
		for k := range h {
			if strings.HasPrefix(http.CanonicalHeaderKey(k), "X-Solinear-") {
				delete(h, k)
			}
		}
		h.Set("X-Solinear-Login", login(pr.In))
		h.Set("X-Solinear-Secret", secret)
	}}
}

// ---- control API ----

func requireToken(token string, next http.Handler) http.Handler {
	want := []byte("Bearer " + token)
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if subtle.ConstantTimeCompare([]byte(r.Header.Get("Authorization")), want) != 1 {
			writeJSON(w, 401, map[string]any{"error": "unauthorized"})
			return
		}
		next.ServeHTTP(w, r)
	})
}

func writeJSON(w http.ResponseWriter, code int, v any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(code)
	json.NewEncoder(w).Encode(v)
}

// group is a set of listeners that are closed together.
type group struct{ cs []io.Closer }

func (g *group) add(c io.Closer) { g.cs = append(g.cs, c) }
func (g *group) close() {
	for _, c := range g.cs {
		c.Close()
	}
	g.cs = nil
}

type whoisEntry struct {
	login string
	at    time.Time
}

type app struct {
	srv *tsnet.Server
	lc  *local.Client

	mu     sync.Mutex // guards host, client
	host   *group
	client *group

	whoisMu    sync.Mutex
	whoisCache map[string]whoisEntry
}

// loginOf returns the tailnet login of a remote ("ip:port" or ip), "" if unknown. Cached 60s per IP.
func (a *app) loginOf(remote string) string {
	ip := remote
	if ap, err := netip.ParseAddrPort(remote); err == nil {
		ip = ap.Addr().String()
	}
	a.whoisMu.Lock()
	e, ok := a.whoisCache[ip]
	a.whoisMu.Unlock()
	if ok && time.Since(e.at) < time.Minute {
		return e.login
	}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	res, err := a.lc.WhoIs(ctx, ip)
	if err != nil || res.UserProfile == nil {
		return "" // not cached: retry next time
	}
	a.whoisMu.Lock()
	a.whoisCache[ip] = whoisEntry{res.UserProfile.LoginName, time.Now()}
	a.whoisMu.Unlock()
	return res.UserProfile.LoginName
}

func plainDial(ctx context.Context, network, addr string) (net.Conn, error) {
	var d net.Dialer
	return d.DialContext(ctx, network, addr)
}

func (a *app) setHost(enabled bool, owner, agent, secret string) error {
	a.mu.Lock()
	defer a.mu.Unlock()
	if a.host != nil {
		a.host.close()
		a.host = nil
	}
	if !enabled {
		return nil
	}
	if owner == "" || agent == "" {
		return errors.New("owner and agent are required")
	}
	target, err := url.Parse("http://" + agent)
	if err != nil {
		return err
	}
	ip4, _ := a.srv.TailscaleIPs()
	if !ip4.IsValid() {
		return errors.New("not connected to the tailnet yet")
	}
	g := &group{}
	allow := func(remote net.Addr) bool { return strings.EqualFold(a.loginOf(remote.String()), owner) }
	fail := func(err error) error { g.close(); return err }
	for _, p := range tcpPorts {
		ln, err := a.srv.Listen("tcp", ":"+strconv.Itoa(p))
		if err != nil {
			return fail(err)
		}
		g.add(ln)
		go forwardTCP(ln, plainDial, "127.0.0.1:"+strconv.Itoa(p), allow)
	}
	for _, p := range udpPorts { // tsnet's ListenPacket needs a concrete tailnet IP, not ":port"
		pc, err := a.srv.ListenPacket("udp", net.JoinHostPort(ip4.String(), strconv.Itoa(p)))
		if err != nil {
			return fail(err)
		}
		g.add(pc)
		go forwardUDP(pc, plainDial, "127.0.0.1:"+strconv.Itoa(p), allow, 60*time.Second)
	}
	ln, err := a.srv.Listen("tcp", ":"+strconv.Itoa(agentPort))
	if err != nil {
		return fail(err)
	}
	hs := &http.Server{Handler: agentProxy(target, secret, func(r *http.Request) string { return a.loginOf(r.RemoteAddr) })}
	g.add(hs) // closes ln too
	go hs.Serve(ln)
	a.host = g
	return nil
}

func (a *app) connect(ip string) error {
	if _, err := netip.ParseAddr(ip); err != nil {
		return err
	}
	a.disconnect()
	a.mu.Lock()
	defer a.mu.Unlock()
	g := &group{}
	fail := func(err error) error { g.close(); return err }
	for _, p := range tcpPorts {
		ln, err := net.Listen("tcp", "[::1]:"+strconv.Itoa(p))
		if err != nil {
			return fail(err)
		}
		g.add(ln)
		go forwardTCP(ln, a.srv.Dial, net.JoinHostPort(ip, strconv.Itoa(p)), nil)
	}
	for _, p := range udpPorts {
		pc, err := net.ListenPacket("udp", "[::1]:"+strconv.Itoa(p))
		if err != nil {
			return fail(err)
		}
		g.add(pc)
		go forwardUDP(pc, a.srv.Dial, net.JoinHostPort(ip, strconv.Itoa(p)), nil, 60*time.Second)
	}
	a.client = g
	return nil
}

func (a *app) disconnect() {
	a.mu.Lock()
	defer a.mu.Unlock()
	if a.client != nil {
		a.client.close()
		a.client = nil
	}
}

func shortName(dns, host string) string {
	if n := strings.SplitN(strings.TrimSuffix(dns, "."), ".", 2)[0]; n != "" {
		return n
	}
	return host
}

func firstV4(ips []netip.Addr) string {
	for _, ip := range ips {
		if ip.Is4() {
			return ip.String()
		}
	}
	return ""
}

func (a *app) status(ctx context.Context) (map[string]any, error) {
	st, err := a.lc.Status(ctx)
	if err != nil {
		return nil, err
	}
	state := "Starting"
	switch st.BackendState {
	case "Running", "NeedsLogin":
		state = st.BackendState
	}
	var authURL, self any
	if st.AuthURL != "" {
		authURL = st.AuthURL
	}
	if s := st.Self; s != nil && state == "Running" {
		self = map[string]string{"name": shortName(s.DNSName, s.HostName), "ip": firstV4(s.TailscaleIPs), "login": st.User[s.UserID].LoginName}
	}
	type peer struct {
		Name   string `json:"name"`
		IP     string `json:"ip"`
		Online bool   `json:"online"`
		OS     string `json:"os"`
	}
	peers := []peer{}
	for _, p := range st.Peer {
		if ip := firstV4(p.TailscaleIPs); ip != "" {
			peers = append(peers, peer{shortName(p.DNSName, p.HostName), ip, p.Online, p.OS})
		}
	}
	sort.Slice(peers, func(i, j int) bool { return peers[i].Name < peers[j].Name })
	return map[string]any{"state": state, "auth_url": authURL, "self": self, "peers": peers}, nil
}

func (a *app) routes() http.Handler {
	mux := http.NewServeMux()
	reply := func(w http.ResponseWriter, v any, err error) {
		if err != nil {
			writeJSON(w, 500, map[string]any{"error": err.Error()})
			return
		}
		writeJSON(w, 200, v)
	}
	decode := func(w http.ResponseWriter, r *http.Request, v any) bool {
		if json.NewDecoder(r.Body).Decode(v) != nil {
			writeJSON(w, 400, map[string]any{"error": "bad json"})
			return false
		}
		return true
	}
	mux.HandleFunc("GET /status", func(w http.ResponseWriter, r *http.Request) {
		v, err := a.status(r.Context())
		reply(w, v, err)
	})
	mux.HandleFunc("POST /login", func(w http.ResponseWriter, r *http.Request) {
		var authURL any
		st, err := a.lc.Status(r.Context())
		if err == nil && st.BackendState != "Running" {
			if st.AuthURL == "" { // after /logout tsnet won't start a login by itself
				err = a.lc.StartLoginInteractive(r.Context())
			}
			for i := 0; err == nil && i < 40; i++ { // up to ~10s for control to hand out a URL
				if st, err = a.lc.Status(r.Context()); err == nil && st.AuthURL != "" {
					authURL = st.AuthURL
					break
				}
				time.Sleep(250 * time.Millisecond)
			}
		}
		reply(w, map[string]any{"auth_url": authURL}, err)
	})
	mux.HandleFunc("POST /logout", func(w http.ResponseWriter, r *http.Request) {
		a.setHost(false, "", "", "")
		a.disconnect()
		reply(w, map[string]any{"ok": true}, a.lc.Logout(r.Context()))
	})
	mux.HandleFunc("GET /whois", func(w http.ResponseWriter, r *http.Request) {
		var login any
		if res, err := a.lc.WhoIs(r.Context(), r.URL.Query().Get("ip")); err == nil && res.UserProfile != nil {
			login = res.UserProfile.LoginName
		}
		writeJSON(w, 200, map[string]any{"login": login})
	})
	mux.HandleFunc("GET /ping", func(w http.ResponseWriter, r *http.Request) {
		ip, err := netip.ParseAddr(r.URL.Query().Get("ip"))
		if err != nil {
			writeJSON(w, 400, map[string]any{"error": "bad ip"})
			return
		}
		ctx, cancel := context.WithTimeout(r.Context(), 5*time.Second)
		defer cancel()
		res, err := a.lc.Ping(ctx, ip, tailcfg.PingDisco)
		out := map[string]any{"direct": false, "ms": nil}
		if err == nil && res.Err == "" {
			out["direct"] = res.Endpoint != ""
			out["ms"] = res.LatencySeconds * 1000
		}
		writeJSON(w, 200, out)
	})
	mux.HandleFunc("POST /host", func(w http.ResponseWriter, r *http.Request) {
		var b struct {
			Enabled              bool
			Owner, Agent, Secret string
		}
		if decode(w, r, &b) {
			reply(w, map[string]any{"ok": true}, a.setHost(b.Enabled, b.Owner, b.Agent, b.Secret))
		}
	})
	mux.HandleFunc("POST /connect", func(w http.ResponseWriter, r *http.Request) {
		var b struct{ IP string }
		if decode(w, r, &b) {
			reply(w, map[string]any{"ok": true}, a.connect(b.IP))
		}
	})
	mux.HandleFunc("POST /disconnect", func(w http.ResponseWriter, r *http.Request) {
		a.disconnect()
		writeJSON(w, 200, map[string]any{"ok": true})
	})
	mux.HandleFunc("/peer/{ip}/agent/{path...}", func(w http.ResponseWriter, r *http.Request) {
		ip, err := netip.ParseAddr(r.PathValue("ip"))
		if err != nil {
			writeJSON(w, 400, map[string]any{"error": "bad ip"})
			return
		}
		u := "http://" + net.JoinHostPort(ip.String(), strconv.Itoa(agentPort)) + "/" + r.PathValue("path")
		if r.URL.RawQuery != "" {
			u += "?" + r.URL.RawQuery
		}
		ctx, cancel := context.WithTimeout(r.Context(), 30*time.Second)
		defer cancel()
		// buffered, so the request carries a Content-Length: older (Python) agents ignore chunked bodies
		body, err := io.ReadAll(io.LimitReader(r.Body, 64<<10))
		if err != nil {
			writeJSON(w, 400, map[string]any{"error": "bad body"})
			return
		}
		req, _ := http.NewRequestWithContext(ctx, r.Method, u, bytes.NewReader(body))
		if ct := r.Header.Get("Content-Type"); ct != "" {
			req.Header.Set("Content-Type", ct)
		}
		resp, err := a.srv.HTTPClient().Do(req)
		if err != nil {
			writeJSON(w, 502, map[string]any{"error": err.Error()})
			return
		}
		defer resp.Body.Close()
		if ct := resp.Header.Get("Content-Type"); ct != "" {
			w.Header().Set("Content-Type", ct)
		}
		w.WriteHeader(resp.StatusCode)
		io.Copy(w, resp.Body)
	})
	return mux
}

func main() {
	state := flag.String("state", "", "tsnet state dir")
	hostname := flag.String("hostname", "solinear", "tailnet hostname")
	control := flag.String("control", "127.0.0.1:0", "local control API address")
	token := flag.String("token", "", "bearer token for the control API")
	loginServer := flag.String("login-server", "", "alternative control server (Headscale)")
	port := flag.Int("port", 41643, "tsnet UDP port")
	flag.Parse()
	log.SetOutput(os.Stderr)
	if *state == "" || *token == "" {
		log.Fatal("--state and --token are required")
	}

	srv := &tsnet.Server{Dir: *state, Hostname: *hostname, ControlURL: *loginServer, Port: uint16(*port), Logf: log.Printf}
	if err := srv.Start(); err != nil {
		log.Fatal(err)
	}
	lc, err := srv.LocalClient()
	if err != nil {
		log.Fatal(err)
	}
	a := &app{srv: srv, lc: lc, whoisCache: map[string]whoisEntry{}}

	ln, err := net.Listen("tcp", *control)
	if err != nil {
		log.Fatal(err)
	}
	fmt.Fprintf(os.Stderr, "solinear-net: control API on %s\n", ln.Addr())
	go http.Serve(ln, requireToken(*token, a.routes()))

	io.Copy(io.Discard, os.Stdin) // parent gone -> stdin closes
	a.setHost(false, "", "", "")
	a.disconnect()
	srv.Close()
}
