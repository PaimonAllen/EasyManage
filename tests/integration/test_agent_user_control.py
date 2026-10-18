"""Pure fallback-control tests; run in disposable namespaces with no host cron."""
import importlib.util
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

spec=importlib.util.spec_from_file_location("user_control",Path(__file__).resolve().parents[2]/"deploy/linux/user/agent-user-control.py")
control=importlib.util.module_from_spec(spec)
spec.loader.exec_module(control)


class ControlTests(unittest.TestCase):
    def test_systemd_preference_and_safe_compatibility_switch_without_linger(self):
        with tempfile.TemporaryDirectory(dir="/home",prefix="compat-fixture-") as temporary:
            home=Path(temporary)
            name=home.name
            bundle=home/".easymanage/staging/agent.ABC123xy"
            bundle.mkdir(parents=True)
            mocks=home/"mock-bin"
            mocks.mkdir()
            code="""#!/usr/bin/python3
import pathlib,sys
name=pathlib.Path(sys.argv[0]).name
if name=='id': print('USER_PLACEHOLDER' if sys.argv[1:] == ['-un'] else 'UID_PLACEHOLDER');sys.exit(0)
if name=='getent': print('USER_PLACEHOLDER:x:UID_PLACEHOLDER:UID_PLACEHOLDER::HOME_PLACEHOLDER:/bin/bash');sys.exit(0)
if name=='loginctl': print('yes');sys.exit(0)
if name=='systemctl':
 if '--user' in sys.argv: sys.exit(0)
 if 'show' in sys.argv: print('inactive');sys.exit(0)
 sys.exit(1)
sys.exit(2)
""".replace('USER_PLACEHOLDER',name).replace('UID_PLACEHOLDER',str(os.getuid())).replace('HOME_PLACEHOLDER',str(home))
            for command in ("id","getent","loginctl","systemctl","crontab"):
                (mocks/command).write_text(code)
                (mocks/command).chmod(0o700)
            source=Path(__file__).resolve().parents[2]/"deploy/linux/user"
            for file in ("agent-user-control.py","install-user-agent.sh","easymanage-agent.user.service"):
                shutil.copyfile(source/file,bundle/file)
            (bundle/"easymanage-agent").write_text("#!/usr/bin/python3\nimport time\nwhile True: time.sleep(1)\n")
            base=home/".easymanage"
            node="11111111-2222-4333-8444-555555555555"
            values={"EASYMANAGE_SERVER_URL":"https://platform.example:8443","EASYMANAGE_CA_CERTIFICATE":str(base/"config/ca.crt"),
                    "EASYMANAGE_AGENT_STATE_DB":str(base/"state/state.db"),"EASYMANAGE_ENROLLMENT_TOKEN":"a"*64,"EASYMANAGE_NODE_ID":node}
            (bundle/"agent.env").write_text("".join(key+"="+value+"\n" for key,value in values.items()))
            (bundle/"node.json").write_text(json.dumps({"node_id":node,"user":name,"agent_home":str(base)}))
            (bundle/"ca.crt").write_text("fixture-ca")
            (bundle/"SHA256SUMS").write_text("".join(hashlib.sha256(path.read_bytes()).hexdigest()+"  "+path.name+"\n" for path in sorted(bundle.iterdir())))
            result=subprocess.run(["/bin/bash",str(bundle/"install-user-agent.sh")],env=os.environ|{"HOME":str(home),"PATH":str(mocks)+":/usr/bin:/bin"},capture_output=True,timeout=15)
            self.assertEqual(result.returncode,0,result.stderr.decode())
            self.assertEqual(json.loads((base/"config/agent-launch.json").read_text()),{"backend":"systemd","autostart":"systemd"})
            self.assertTrue((home/".config/systemd/user/easymanage-agent.service").is_file())
            self.assertFalse((base/"state/compat.enabled").exists())
            (mocks/"loginctl").write_text("#!/bin/sh\nprintf 'no\\n'\n")
            environment=os.environ|{"HOME":str(home),"PATH":str(mocks)+":/usr/bin:/bin"}
            try:
                result=subprocess.run(["/bin/bash",str(bundle/"install-user-agent.sh")],env=environment,capture_output=True,timeout=20)
                self.assertEqual(result.returncode,0,result.stderr.decode())
                self.assertEqual(json.loads((base/"config/agent-launch.json").read_text()),{"backend":"compatibility","autostart":"manual"})
                self.assertFalse((home/".config/systemd/user/easymanage-agent.service").exists())
                self.assertTrue(list(base.glob("install-backup.*/retired-user-unit")))
            finally:
                subprocess.run(["/usr/bin/python3",str(base/"bin/agent-user-control.py"),"stop"],env=environment,timeout=25,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)

    def test_cron_preserves_other_jobs_and_only_replaces_its_own_block(self):
        with patch.object(control,"BASE",Path("/home/fixture/.easymanage")):
            original="# user job\n0 4 * * * /home/fixture/backup\n"
            updated=control.with_managed_cron(original)
            self.assertTrue(updated.startswith(original))
            self.assertIn("@reboot ",updated)
            self.assertIn("start --enabled-only",updated)
            self.assertEqual(control.with_managed_cron(updated),updated)
            self.assertEqual(updated.count(control.BEGIN),1)
            with self.assertRaises(RuntimeError): control.with_managed_cron(control.BEGIN+"\nforeign\n")

    def test_unreadable_or_denied_crontab_is_never_replaced(self):
        for result in (SimpleNamespace(returncode=1,stdout="",stderr="not allowed to use this program"),
                       SimpleNamespace(returncode=2,stdout="",stderr="permission denied")):
            with patch.object(control.shutil,"which",return_value="/usr/bin/crontab"),patch.object(control.subprocess,"run",return_value=result):
                self.assertIsNone(control.cron_table())

    def test_absent_cron_is_an_honest_manual_backend(self):
        with tempfile.TemporaryDirectory() as temporary,patch.object(control,"BASE",Path(temporary)),patch.object(control,"cron_table",return_value=None):
            (Path(temporary)/"config").mkdir()
            control.configure()
            self.assertEqual(json.loads((Path(temporary)/"config/agent-launch.json").read_text()),{"backend":"compatibility","autostart":"manual"})

    def test_disabled_watchdog_does_not_reenable_or_spawn(self):
        with tempfile.TemporaryDirectory() as temporary,patch.object(control,"BASE",Path(temporary)),patch.object(control.subprocess,"Popen") as spawn:
            control.start(enabled_only=True)
            spawn.assert_not_called()
            self.assertFalse((Path(temporary)/"state/compat.enabled").exists())

    def test_pid_reuse_never_targets_an_unrelated_process(self):
        with tempfile.TemporaryDirectory() as temporary,patch.object(control,"BASE",Path(temporary)),patch.object(control.os,"kill") as kill:
            (Path(temporary)/"state").mkdir()
            (Path(temporary)/"state/compat-process.json").write_text(json.dumps({"pid":999999999,"start":"0"}))
            control.stop()
            kill.assert_not_called()


if __name__=="__main__": unittest.main()
