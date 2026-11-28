package main

import (
	"context"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"net/url"
	"testing"
	"time"
)

func TestRequireToken(t *testing.T) {
	h := requireToken("s3cret", http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { w.WriteHeader(204) }))
	for auth, want := range map[string]int{"": 401, "Bearer nope": 401, "s3cret": 401, "Bearer s3cret": 204} {
		r := httptest.NewRequest("GET", "/status", nil)
		if auth != "" {
			r.Header.Set("Authorization", auth)
		}
		w := httptest.NewRecorder()
		h.ServeHTTP(w, r)
		if w.Code != want {
			t.Errorf("auth %q: got %d want %d", auth, w.Code, want)
		}
	}
}

func TestAgentProxyHeaders(t *testing.T) {
	var got http.Header
	agent := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { got = r.Header.Clone() }))
	defer agent.Close()
	target, _ := url.Parse(agent.URL)

	for _, login := range []string{"me@x.com", ""} {
		p := httptest.NewServer(agentProxy(target, "sec", func(*http.Request) string { return login }))
		req, _ := http.NewRequest("GET", p.URL+"/info", nil)
		req.Header.Set("X-Solinear-Login", "owner@evil")
		req.Header.Set("x-solinear-secret", "guess")
		req.Header.Set("X-Solinear-Other", "1")
		req.Header.Set("X-Keep", "yes")
		resp, err := http.DefaultClient.Do(req)
		if err != nil {
			t.Fatal(err)
		}
		resp.Body.Close()
		p.Close()
		if got.Get("X-Solinear-Login") != login || got.Get("X-Solinear-Secret") != "sec" ||
			got.Get("X-Solinear-Other") != "" || got.Get("X-Keep") != "yes" {
			t.Errorf("login %q: bad headers %v", login, got)
		}
		if len(got.Values("X-Solinear-Login")) != 1 {
			t.Errorf("duplicate login header: %v", got.Values("X-Solinear-Login"))
		}
	}
}

func udpEcho(t *testing.T, prefix string) (addr string, stop func()) {
	pc, err := net.ListenPacket("udp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	go func() {
		b := make([]byte, 2048)
		for {
			n, from, err := pc.ReadFrom(b)
			if err != nil {
				return
			}
			pc.WriteTo(append([]byte(prefix), b[:n]...), from)
		}
	}()
	return pc.LocalAddr().String(), func() { pc.Close() }
}

func TestForwardUDP(t *testing.T) {
	backend, stop := udpEcho(t, "echo:")
	defer stop()
	front, _ := net.ListenPacket("udp", "127.0.0.1:0")
	defer front.Close()
	go forwardUDP(front, plainDial, backend, nil, 300*time.Millisecond)

	ask := func(msg string) string {
		c, err := net.Dial("udp", front.LocalAddr().String())
		if err != nil {
			t.Fatal(err)
		}
		defer c.Close()
		c.SetDeadline(time.Now().Add(2 * time.Second))
		c.Write([]byte(msg))
		b := make([]byte, 64)
		n, err := c.Read(b)
		if err != nil {
			t.Fatal(err)
		}
		return string(b[:n])
	}
	// two independent remotes each get their own reply
	if a, b := ask("one"), ask("two"); a != "echo:one" || b != "echo:two" {
		t.Fatalf("got %q %q", a, b)
	}

	// same remote, several packets, one session; then idle expiry and a fresh session works
	c, _ := net.Dial("udp", front.LocalAddr().String())
	defer c.Close()
	c.SetDeadline(time.Now().Add(3 * time.Second))
	b := make([]byte, 64)
	for _, m := range []string{"a", "b"} {
		c.Write([]byte(m))
		n, err := c.Read(b)
		if err != nil || string(b[:n]) != "echo:"+m {
			t.Fatalf("got %q %v", b[:n], err)
		}
	}
	time.Sleep(600 * time.Millisecond)
	c.Write([]byte("c"))
	if n, err := c.Read(b); err != nil || string(b[:n]) != "echo:c" {
		t.Fatalf("after idle: %q %v", b[:n], err)
	}
}

func TestForwardUDPDeniesAndFailedDial(t *testing.T) {
	backend, stop := udpEcho(t, "")
	defer stop()
	front, _ := net.ListenPacket("udp", "127.0.0.1:0")
	defer front.Close()
	go forwardUDP(front, plainDial, backend, func(net.Addr) bool { return false }, time.Second)
	c, _ := net.Dial("udp", front.LocalAddr().String())
	defer c.Close()
	c.SetReadDeadline(time.Now().Add(300 * time.Millisecond))
	c.Write([]byte("x"))
	if _, err := c.Read(make([]byte, 8)); err == nil {
		t.Fatal("denied packet was forwarded")
	}
}

func TestForwardTCP(t *testing.T) {
	back, _ := net.Listen("tcp", "127.0.0.1:0")
	defer back.Close()
	go func() {
		for {
			c, err := back.Accept()
			if err != nil {
				return
			}
			go func() { io.Copy(c, c); c.Close() }()
		}
	}()
	for _, allow := range []bool{true, false} {
		front, _ := net.Listen("tcp", "127.0.0.1:0")
		go forwardTCP(front, func(ctx context.Context, n, a string) (net.Conn, error) { return plainDial(ctx, n, a) },
			back.Addr().String(), func(net.Addr) bool { return allow })
		c, err := net.Dial("tcp", front.Addr().String())
		if err != nil {
			t.Fatal(err)
		}
		c.SetDeadline(time.Now().Add(time.Second))
		c.Write([]byte("hi"))
		b := make([]byte, 2)
		_, err = io.ReadFull(c, b)
		if allow && (err != nil || string(b) != "hi") {
			t.Fatalf("allowed: %q %v", b, err)
		}
		if !allow && err == nil {
			t.Fatal("denied connection was forwarded")
		}
		c.Close()
		front.Close()
	}
}
