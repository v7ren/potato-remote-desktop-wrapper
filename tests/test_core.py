import json
import unittest
import urllib.error
import urllib.request

from solinear import host, quality, tailnet


class TailnetParsing(unittest.TestCase):
    def test_up_json_blocks(self):
        out = ['{\n', '\t"AuthURL": "https://login.tailscale.com/a/abc",\n', '\t"BackendState": "NeedsLogin"\n', '}\n',
               '{\n', '\t"BackendState": "Running"\n', '}\n']
        blocks = list(tailnet.parse_json_blocks(out))
        self.assertEqual(blocks[0]["AuthURL"], "https://login.tailscale.com/a/abc")
        self.assertEqual(blocks[1]["BackendState"], "Running")

    def test_ping_direct(self):
        text = "pong from gamer (100.101.1.2) via DERP(fra) in 40ms\npong from gamer (100.101.1.2) via 192.168.1.5:41642 in 3ms\n"
        self.assertEqual(tailnet.parse_ping(text), (True, 3.0))

    def test_ping_relayed(self):
        self.assertEqual(tailnet.parse_ping("pong from gamer (100.1.1.1) via DERP(fra) in 38ms"), (False, 38.0))

    def test_ping_none(self):
        self.assertEqual(tailnet.parse_ping("timeout"), (False, None))


class Quality(unittest.TestCase):
    def test_1080p60_about_20mbps(self):
        self.assertAlmostEqual(quality.bitrate_kbps(1920, 1080, 60, True), 19_906, delta=50)

    def test_relay_capped(self):
        self.assertEqual(quality.bitrate_kbps(3840, 2160, 120, False), quality.RELAY_CAP)

    def test_display_mode_and_scroll_flags(self):
        f = quality.flags(1920, 1080, 60, 20000)
        self.assertEqual(f[f.index("--display-mode") + 1], "borderless")
        self.assertIn("--no-reverse-scroll-direction", f)
        f = quality.flags(1920, 1080, 60, 20000, "windowed", True)
        self.assertEqual(f[f.index("--display-mode") + 1], "windowed")
        self.assertIn("--reverse-scroll-direction", f)
        f = quality.flags(1920, 1080, 60, 20000, "bogus")  # bad saved value falls back
        self.assertEqual(f[f.index("--display-mode") + 1], "borderless")

    def test_choice_scales(self):
        auto = quality.bitrate_kbps(1920, 1080, 60, True)
        self.assertLess(quality.bitrate_kbps(1920, 1080, 60, True, "smoother"), auto)
        self.assertGreater(quality.bitrate_kbps(1920, 1080, 60, True, "sharper"), auto)


class ServerIdPinned(unittest.TestCase):
    """The host ID must stay the same across restarts and survive a wiped state file."""

    def setUp(self):
        import tempfile
        from pathlib import Path
        self.orig = host.HOME
        host.HOME = Path(tempfile.mkdtemp())
        self.state = host.HOME / "state.json"

    def tearDown(self):
        host.HOME = self.orig

    def test_adds_id_keeps_creds(self):
        self.state.write_text(json.dumps({"username": "solinear", "salt": "x", "password": "y"}))
        cfg = {}
        host.pin_server_id(cfg)
        data = json.loads(self.state.read_text())
        self.assertEqual(data["root"]["uniqueid"], cfg["server_id"])
        self.assertEqual(data["username"], "solinear")

    def test_stable_and_restored_after_wipe(self):
        cfg = {}
        host.pin_server_id(cfg)
        first = cfg["server_id"]
        host.pin_server_id(cfg)
        self.state.unlink()
        host.pin_server_id(cfg)
        self.assertEqual(json.loads(self.state.read_text())["root"]["uniqueid"], first)

    def test_existing_server_id_adopted(self):
        self.state.write_text(json.dumps({"root": {"uniqueid": "ABC-1", "named_devices": []}}))
        cfg = {}
        host.pin_server_id(cfg)
        self.assertEqual(cfg["server_id"], "ABC-1")


class KeyRemap(unittest.TestCase):
    """Mac Cmd+C must land as Ctrl+C on a Windows host (and Ctrl as Cmd on a Mac host); same OS: untouched."""

    def setUp(self):
        import tempfile
        from pathlib import Path
        self.orig = host.CONF, host.store.MAC, host.store.save
        host.CONF = Path(tempfile.mkdtemp()) / "sunshine.conf"
        host.store.save = lambda cfg: None

    def tearDown(self):
        host.CONF, host.store.MAC, host.store.save = self.orig

    def test_mac_client_on_windows_host(self):
        host.store.MAC = False
        m = dict(zip(host.keymap("mac")[::2], host.keymap("mac")[1::2]))
        self.assertEqual(m[0x5B], 0xA2)  # Cmd -> Ctrl
        self.assertEqual(host.keymap("windows"), [])

    def test_windows_client_on_mac_host(self):
        host.store.MAC = True
        m = dict(zip(host.keymap("windows")[::2], host.keymap("windows")[1::2]))
        self.assertEqual(m[0xA2], 0x5B)  # Ctrl -> Cmd
        self.assertEqual(host.keymap("mac"), [])

    def test_restart_only_when_mapping_changes(self):
        host.store.MAC = False
        cfg = {}
        self.assertFalse(host.set_keys(cfg, "windows"))  # same OS: nothing to do
        self.assertTrue(host.set_keys(cfg, "mac"))
        self.assertIn("keybindings = [0x5B, 0xA2,", host.CONF.read_text())
        self.assertFalse(host.set_keys(cfg, "mac"))
        self.assertTrue(host.set_keys(cfg, ""))  # toggle off: back to no remap
        self.assertNotIn("keybindings", host.CONF.read_text())


class SunshinePairingPick(unittest.TestCase):
    """Sunshine needs the pending request's id; only the request from the asking client's IP may be used."""

    def test_matches_ip_including_ipv6_mapped(self):
        pend = [{"id": "a" * 32, "address": "100.1.1.1"}, {"id": "b" * 32, "address": "::ffff:100.2.2.2"}]
        self.assertEqual(host.pick_pairing(pend, "100.2.2.2"), "b" * 32)

    def test_other_clients_request_not_used(self):
        self.assertIsNone(host.pick_pairing([{"id": "a" * 32, "address": "100.9.9.9"}], "100.2.2.2"))


class PairingAgent(unittest.TestCase):
    """The security boundary: only the owner's devices may submit a PIN."""

    def setUp(self):
        self.submitted = []
        self.actions = []
        self.whois_login = "owner@gmail.com"
        handler = host.make_handler(owner="owner@gmail.com", whois=lambda ip: self.whois_login,
                                    submit=lambda pin, name, ip: self.submitted.append((pin, name)) or True,
                                    info={"name": "GAMER"},
                                    action=lambda name: self.actions.append(name) or True,
                                    keys=lambda client_os: self.actions.append("keys:" + client_os) or True)
        orig = host.AGENT_PORT
        host.AGENT_PORT = 0  # pick a free port
        self.server = host.serve("127.0.0.1", handler)
        host.AGENT_PORT = orig
        self.url = f"http://127.0.0.1:{self.server.server_address[1]}"

    def tearDown(self):
        self.server.shutdown()
        self.server.server_close()

    def post(self, body):
        req = urllib.request.Request(self.url + "/pair", json.dumps(body).encode(), {"Content-Type": "application/json"})
        with urllib.request.urlopen(req, timeout=3) as r:
            return json.load(r)

    def test_info(self):
        with urllib.request.urlopen(self.url + "/info", timeout=3) as r:
            self.assertEqual(json.load(r)["name"], "GAMER")

    def test_owner_can_pair(self):
        self.assertEqual(self.post({"pin": "0420", "name": "laptop"}), {"status": True})
        self.assertEqual(self.submitted, [("0420", "laptop")])

    def test_other_account_refused(self):
        self.whois_login = "stranger@gmail.com"
        with self.assertRaises(urllib.error.HTTPError) as e:
            self.post({"pin": "0420"})
        self.assertEqual(e.exception.code, 403)
        self.assertEqual(self.submitted, [])

    def test_unknown_peer_refused(self):
        self.whois_login = None
        with self.assertRaises(urllib.error.HTTPError) as e:
            self.post({"pin": "0420"})
        self.assertEqual(e.exception.code, 403)

    def test_cancel_only_for_owner(self):
        req = urllib.request.Request(self.url + "/cancel", b"{}", {"Content-Type": "application/json"})
        with urllib.request.urlopen(req, timeout=3) as r:
            self.assertEqual(json.load(r), {"status": True})
        self.whois_login = "stranger@gmail.com"
        with self.assertRaises(urllib.error.HTTPError) as e:
            urllib.request.urlopen(req, timeout=3)
        self.assertEqual(e.exception.code, 403)

    def test_action_owner_only_and_known_names_only(self):
        def act(name):
            req = urllib.request.Request(self.url + "/action", json.dumps({"action": name}).encode(),
                                         {"Content-Type": "application/json"})
            with urllib.request.urlopen(req, timeout=3) as r:
                return json.load(r)

        self.assertEqual(act("lock"), {"status": True})
        self.assertEqual(self.actions, ["lock"])
        with self.assertRaises(urllib.error.HTTPError) as e:
            act("format c:")
        self.assertEqual(e.exception.code, 400)
        self.whois_login = "stranger@gmail.com"
        with self.assertRaises(urllib.error.HTTPError) as e:
            act("shutdown")
        self.assertEqual(e.exception.code, 403)
        self.assertEqual(self.actions, ["lock"])

    def test_keys_owner_only_and_known_os_only(self):
        def keys(os_name):
            req = urllib.request.Request(self.url + "/keys", json.dumps({"client_os": os_name}).encode(),
                                         {"Content-Type": "application/json"})
            with urllib.request.urlopen(req, timeout=3) as r:
                return json.load(r)

        self.assertEqual(keys("mac"), {"status": True, "restarted": True})
        with self.assertRaises(urllib.error.HTTPError) as e:
            keys("linux; rm -rf")
        self.assertEqual(e.exception.code, 400)
        self.whois_login = "stranger@gmail.com"
        with self.assertRaises(urllib.error.HTTPError) as e:
            keys("windows")
        self.assertEqual(e.exception.code, 403)
        self.assertEqual(self.actions, ["keys:mac"])

    def test_bad_pin_rejected(self):
        for pin in ["12", "abcd", "12345", ""]:
            with self.assertRaises(urllib.error.HTTPError) as e:
                self.post({"pin": pin})
            self.assertEqual(e.exception.code, 400)
        self.assertEqual(self.submitted, [])


if __name__ == "__main__":
    unittest.main()
