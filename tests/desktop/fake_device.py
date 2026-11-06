"""A stand-in device for the desktop key tests: a TCP server that records every byte it gets."""
import socket
import threading
import time


class FakeDevice:
    def __init__(self):
        self.received = bytearray()
        self.lock = threading.Lock()
        self.connected = threading.Event()
        self.closed = threading.Event()
        self.results = []
        listener = socket.socket()
        listener.bind(("127.0.0.1", 0))
        listener.listen(1)
        self.port = listener.getsockname()[1]
        threading.Thread(target=self._serve, args=(listener,), daemon=True).start()

    def _serve(self, listener):
        conn, _ = listener.accept()
        self.connected.set()
        conn.sendall(b"test device ready\r\nrouter> ")
        while True:
            data = conn.recv(4096)
            if not data:
                self.closed.set()
                return
            with self.lock:
                self.received.extend(data)

    def take(self):
        with self.lock:
            data = bytes(self.received)
            self.received.clear()
        return data

    def record(self, name, ok, detail=""):
        print(f"{'PASS' if ok else 'FAIL'}  {name}{detail}", flush=True)
        self.results.append((name, ok))
        return ok

    def check(self, name, press, wanted, settle=1.5):
        """Runs `press` and checks the device got exactly `wanted`, and nothing more."""
        self.take()
        press()
        deadline = time.time() + 8
        while time.time() < deadline:
            time.sleep(0.1)
            with self.lock:
                if len(self.received) >= max(len(wanted), 1):
                    break
        time.sleep(settle)
        got = self.take()
        ok = got == wanted
        return self.record(name, ok, f": sent {got!r}" + ("" if ok else f", wanted {wanted!r}"))

    def check_closed(self, name):
        return self.record(name, self.closed.wait(10))

    def report(self):
        failed = [n for n, ok in self.results if not ok]
        print(f"{len(self.results) - len(failed)} passed, {len(failed)} failed")
        return 1 if failed else 0
