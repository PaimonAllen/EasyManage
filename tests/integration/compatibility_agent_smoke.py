#!/usr/bin/env python3
"""Detached user-process lifecycle with fake Agent and private cron/systemctl."""
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import time

assert len(sys.argv)==3
for namespace,original in zip(("net","mnt"),sys.argv[1:]):
    assert os.readlink(f"/proc/self/ns/{namespace}")!=original
assert os.geteuid()!=0

with tempfile.TemporaryDirectory(dir="/tmp",prefix="compat-agent-test.") as temporary:
    home=Path(temporary)
    base=home/".easymanage"
    (base/"bin").mkdir(parents=True)
    commands=home/"commands"
    commands.mkdir()
    (commands/"systemctl").write_text("#!/bin/sh\nexit 1\n")
    (commands/"systemctl").chmod(0o700)
    helper=base/"bin/agent-user-control.py"
    shutil.copyfile("deploy/linux/user/agent-user-control.py",helper)
    fake=base/"bin/easymanage-agent"
    fake.write_text('''#!/usr/bin/python3
import os,pathlib,signal,time
base=pathlib.Path(os.environ['HOME'])/'.easymanage'
with (base/'starts').open('a') as log: log.write(str(os.getpid())+'\\n')
signal.signal(signal.SIGTERM,lambda *_:exit(0))
os.write(1,b'x'*(3*1024*1024))
while True: time.sleep(1)
''')
    fake.chmod(0o700)
    environment={"HOME":str(home),"PATH":str(commands)+":/usr/bin:/bin","USER":"fixture"}
    def run(action,*extra):
        subprocess.run(["/usr/bin/python3",str(helper),action,*extra],env=environment,check=True,timeout=25,stdout=subprocess.DEVNULL)
    def state(): return json.loads((base/"state/compat-process.json").read_text())
    def agent():
        deadline=time.monotonic()+5
        while time.monotonic()<deadline:
            value=state()
            if value.get("agent_pid"): return value
            time.sleep(0.05)
        raise AssertionError("Agent child was not started")
    try:
        run("start")
        first=agent()
        run("start")
        assert state()["pid"]==first["pid"]
        deadline=time.monotonic()+5
        while time.monotonic()<deadline:
            if (base/"logs/agent.log.1").exists(): break
            time.sleep(0.05)
        assert (base/"logs/agent.log").stat().st_size<=1024*1024
        assert (base/"logs/agent.log.1").stat().st_size<=1024*1024
        print("PASS compatibility singleton and bounded private log rotation",flush=True)
        os.kill(first["pid"],signal.SIGKILL)
        time.sleep(0.25)
        run("start","--enabled-only")
        assert (base/"starts").read_text().splitlines()==[str(first["agent_pid"])]
        run("status")
        print("PASS orphan Agent retains singleton lock; watchdog cannot duplicate it",flush=True)
        run("stop")
        assert not (base/"state/compat.enabled").exists()
        run("start","--enabled-only")
        assert (base/"starts").read_text().splitlines()==[str(first["agent_pid"])]
        print("PASS stop safely handles orphan PID and disables cron resurrection",flush=True)
        run("start")
        second=agent()
        assert second["pid"]!=first["pid"] and second["agent_pid"]!=first["agent_pid"]
        run("restart")
        assert agent()["pid"]!=second["pid"]
        print("PASS explicit start/restart works without systemd user manager or linger",flush=True)
    finally:
        run("stop")
