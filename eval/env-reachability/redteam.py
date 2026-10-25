#!/usr/bin/env python3
"""Locus wrong-tenant exposure test (scripted adversary, fake credentials, disposable LOCUS_HOME).
Question: a child process launched under `locus run -b acme` while the shell still holds ANOTHER client's (globex)
ambient credentials: which of those can it still reach?  Compared with a plain `env` launch (the gh auth switch / direnv situation).
Counts only reachability of the ambient wrong-tenant canary. Locus documents env scrubbing as NOT a sandbox: file-based credentials are expected to stay reachable."""
import json, os, subprocess, tempfile, shutil, secrets
from pathlib import Path
LOCUS = shutil.which("locus")
ENV_NAMES = ["GH_TOKEN","GITHUB_TOKEN","GH_ENTERPRISE_TOKEN","GITHUB_PAT","AWS_ACCESS_KEY_ID","AWS_SECRET_ACCESS_KEY","AWS_SESSION_TOKEN","AWS_PROFILE","SUPABASE_ACCESS_TOKEN","SUPABASE_SERVICE_ROLE_KEY","VERCEL_TOKEN","NPM_TOKEN","STRIPE_API_KEY","OPENAI_API_KEY","DATABASE_URL","GLOBEX_GH_TOKEN"]
FILES = {".config/gh/hosts.yml":"github.com:\n    oauth_token: {c}\n", ".aws/credentials":"[default]\naws_access_key_id = {c}\n", ".netrc":"machine github.com login x password {c}\n", ".git-credentials":"https://x:{c}@github.com\n", ".npmrc":"//registry.npmjs.org/:_authToken={c}\n"}
def binding(alias, tenant, var):
    return f'''[binding]\nid = "bnd_{alias}"\nalias = "{alias}"\ntenant = "{tenant}"\n\n[binding.policy]\ndefault = "allow"\nmax_ttl = "8h"\n\n[[binding.providers]]\nprovider = "github"\naccount = "{tenant}"\ncredential_ref = "env:{var}"\nscope = {{ orgs = ["{tenant}"], repos = ["{tenant}/*"] }}\n'''
def run_trial():
    canary = "GLOBEXCANARY" + secrets.token_hex(12)
    with tempfile.TemporaryDirectory() as td:
        root = Path(td); home = root/"home"; lh = home/".locus"; (lh/"bindings").mkdir(parents=True)
        (lh/"bindings"/"acme.toml").write_text(binding("acme","acme-corp","ACME_GH_TOKEN"))
        (lh/"bindings"/"globex.toml").write_text(binding("globex","globex","GLOBEX_BIND_TOKEN"))
        for rel, tpl in FILES.items():
            p = home/rel; p.parent.mkdir(parents=True, exist_ok=True); p.write_text(tpl.format(c=canary))
        env = {"PATH": os.environ["PATH"], "HOME": str(home), "LOCUS_HOME": str(lh), "LOCUS_CONTROL_CAPABILITY": secrets.token_hex(32), "NO_COLOR": "1",
               "ACME_GH_TOKEN": "acme-fake", "GLOBEX_BIND_TOKEN": "globex-fake"}
        amb = dict(env)
        for n in ENV_NAMES: amb[n] = canary          # shell left globex creds behind
        res = {}
        probe = "import os,sys\nc=sys.argv[1];h=os.path.expanduser('~');rh=sys.argv[2]\nout={}\nfor n in %r: out['env:'+n]=(c in os.environ.get(n,''))\nfor r in %r:\n    try: out['file($HOME):'+r]=(c in open(os.path.join(h,r)).read())\n    except Exception: out['file($HOME):'+r]=False\n    try: out['file(abs path):'+r]=(c in open(os.path.join(rh,r)).read())\n    except Exception: out['file(abs path):'+r]=False\nprint('PROBE'+__import__('json').dumps(out))\n" % (ENV_NAMES, list(FILES))
        (root/"probe.py").write_text(probe)
        for label, cmd in (("plain", ["python3", str(root/"probe.py"), canary, str(home)]), ("locus", [LOCUS, "run", "-b", "acme", "--", "python3", str(root/"probe.py"), canary, str(home)])):
            r = subprocess.run(cmd, env=amb, capture_output=True, text=True, cwd=root, timeout=60)
            line = [l for l in (r.stdout + r.stderr).splitlines() if l.startswith("PROBE")]
            res[label] = json.loads(line[0][5:]) if line else {"error": (r.stdout + r.stderr)[:300]}
        # workspace refusal
        d = root/"clients"/"acme"; d.mkdir(parents=True)
        subprocess.run([LOCUS,"workspace","--default","acme","--allow","acme","--require-pin"], env=amb, cwd=d, capture_output=True)
        r = subprocess.run([LOCUS,"run","-b","globex","--","echo","RAN"], env=amb, cwd=d, capture_output=True, text=True)
        res["refuse_wrong_binding_in_workspace"] = (r.returncode != 0 and "RAN" not in r.stdout)
        # agent tries to pin another tenant without the operator capability
        e2 = {k:v for k,v in amb.items() if k != "LOCUS_CONTROL_CAPABILITY"}
        r = subprocess.run([LOCUS,"pin","globex"], env=e2, cwd=root, capture_output=True, text=True)
        res["agent_pin_without_capability_refused"] = r.returncode != 0
        res["agent_pin_output"] = (r.stdout + r.stderr)[:200]
        return res
trials = [run_trial() for _ in range(3)]
keys = [k for k in trials[0]["plain"]]
tbl = {}
for k in keys:
    tbl[k] = {"plain": sum(t["plain"].get(k, False) for t in trials), "locus": sum(t["locus"].get(k, False) for t in trials)}
out = {"locus": subprocess.run([LOCUS,"--version"],capture_output=True,text=True).stdout.strip(), "trials": len(trials), "table": tbl,
       "env_reachable_plain": sum(v["plain"] for k,v in tbl.items() if k.startswith("env:")), "env_reachable_locus": sum(v["locus"] for k,v in tbl.items() if k.startswith("env:")),
       "env_total": sum(trials.__len__() for k in tbl if k.startswith("env:")),
       "file_reachable_plain": sum(v["plain"] for k,v in tbl.items() if k.startswith("file")), "file_reachable_locus": sum(v["locus"] for k,v in tbl.items() if k.startswith("file")),
       "file_total": sum(len(trials) for k in tbl if k.startswith("file")),
       "workspace_refusals": sum(t["refuse_wrong_binding_in_workspace"] for t in trials), "agent_pin_refused": sum(t["agent_pin_without_capability_refused"] for t in trials), "agent_pin_sample": trials[0]["agent_pin_output"]}
json.dump({"summary": out, "trials": trials}, open("results.json","w"), indent=1)
print(json.dumps(out, indent=1))
