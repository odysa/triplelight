"""Guard boundaries and owned-child termination, independent of DuckDB."""
import signal
import subprocess
import sys
import tempfile
from pathlib import Path
import unittest
import supervise_import as guard

class GuardTests(unittest.TestCase):
    def test_bounds_and_lock(self):
        g=guard.GIB
        self.assertIsNone(guard.reason('mine','mine',39*g,49*g))
        self.assertIn('scratch',guard.reason('mine','mine',40*g,49*g))
        self.assertIn('free',guard.reason('mine','mine',39*g,48*g))
        self.assertIn('lock',guard.reason('mine',None,0,100*g))
    def test_stop_owned_child(self):
        child=subprocess.Popen([sys.executable,'-c','import time; time.sleep(60)'],start_new_session=True)
        try:
            guard.stop_owned(child)
            self.assertEqual(child.returncode,-signal.SIGTERM)
            guard.stop_owned(child) # Already exited: no unrelated PID is signalled.
        finally:
            if child.poll() is None:child.kill();child.wait()

if __name__=='__main__':unittest.main()
