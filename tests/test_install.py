"""Installer contract tests, using local release archives and temporary installs."""
import hashlib
import io
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
TARGET = 'x86_64-unknown-linux-gnu'

class InstallerTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.assets = self.root/'assets'
        self.assets.mkdir()
        self.install = self.root/'installed bridge'
        package = self.root/'stage/stream-bridge'
        (package/'bin').mkdir(parents=True)
        (package/'configs').mkdir()
        (package/'.stream-bridge-install').write_text('v0.1.0\n')
        binary = package/'bin/stream-bridge-server'
        binary.write_text('#!/bin/bash\nprintf "%s\\n" "${BRIDGE_TOKEN:-}"\n')
        binary.chmod(0o755)
        for name in ['setup.sh','setup-dashboard.sh','run.sh']:
            shutil.copy2(ROOT/name,package/name)
        for name in ['config.toml','config.receiver.toml','config.shred-only.toml','config.private-source.toml','config.private-receiver.toml','config.shared-receiver.toml']:
            shutil.copy2(ROOT/name,package/'configs'/name)
        self.archive = self.assets/f'stream-bridge-server-{TARGET}.tar.gz'
        with tarfile.open(self.archive,'w:gz') as tar:
            tar.add(package,arcname='stream-bridge')
        self.checksum()

    def checksum(self):
        checksum=hashlib.sha256(self.archive.read_bytes()).hexdigest()
        (self.assets/'SHA256SUMS').write_text(f'{checksum}  {self.archive.name}\n')

    def install_package(self,role='source'):
        return subprocess.run(['bash',str(ROOT/'install.sh')],env=dict(os.environ,BASE_URL=self.assets.as_uri(),TARGET=TARGET,INSTALL_DIR=str(self.install),ROLE=role),text=True,capture_output=True)

    def test_install_upgrade_preserves_user_data_and_token_is_not_shell(self):
        result=self.install_package();self.assertEqual(result.returncode,0,result.stderr)
        (self.install/'config.toml').write_text('personal config\n')
        (self.install/'custom.toml').write_text('custom\n')
        marker=self.root/'must-not-exist'
        token=f'$(touch "{marker}")'
        (self.install/'.bridge-token').write_text(token)
        (self.install/'.bridge-token').chmod(0o600)
        (self.install/'.dashboard-initial-credentials').write_text('sb_test01\ndashboard-secret\n')
        (self.install/'.dashboard-initial-credentials').chmod(0o600)
        (self.install/'dashboard-admin.toml').write_text('credential hash state')
        (self.install/'dashboard-admin.toml').chmod(0o600)
        (self.install/'dashboard-state.toml').write_text('managed state')
        (self.install/'dashboard-state.toml').chmod(0o600)
        (self.install/'certs').mkdir();(self.install/'certs/private.pem').write_text('test certificate')
        result=self.install_package();self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual((self.install/'config.toml').read_text(),'personal config\n')
        self.assertTrue((self.install/'custom.toml').exists())
        self.assertEqual((self.install/'.dashboard-initial-credentials').read_text(),'sb_test01\ndashboard-secret\n')
        self.assertEqual((self.install/'dashboard-state.toml').read_text(),'managed state')
        self.assertEqual((self.install/'dashboard-admin.toml').read_text(),'credential hash state')
        self.assertEqual((self.install/'dashboard-admin.toml').stat().st_mode&0o777,0o600)
        self.assertEqual((self.install/'dashboard-state.toml').stat().st_mode&0o777,0o600)
        self.assertTrue((self.install/'certs/private.pem').exists())
        self.assertEqual((self.install/'.bridge-token').stat().st_mode&0o777,0o600)
        env=dict(os.environ);env.pop('BRIDGE_TOKEN',None)
        result=subprocess.run([str(self.install/'run.sh'),'--check'],env=env,text=True,capture_output=True,check=True)
        self.assertEqual(result.stdout.strip(),token)
        self.assertFalse(marker.exists())
        self.assertEqual(len(list(self.root.glob('installed bridge.backup.*'))),1)

    def test_dashboard_setup_and_credential_loading_are_safe(self):
        result=self.install_package();self.assertEqual(result.returncode,0,result.stderr)
        assets=self.install/'dashboard/dist';assets.mkdir(parents=True)
        (assets/'index.html').write_text('<html></html>')
        script=self.install/'setup-dashboard.sh'
        first=subprocess.run([str(script)],check=True,capture_output=True,text=True)
        token=(self.install/'.dashboard-initial-credentials').read_bytes()
        lines=token.decode().splitlines()
        self.assertRegex(lines[0],r'^sb_[0-9a-f]{6}$')
        self.assertEqual(len(lines[1]),48)
        self.assertIn(lines[0],first.stdout)
        self.assertIn(lines[1],first.stdout)
        again=subprocess.run([str(script)],check=True,capture_output=True,text=True)
        self.assertNotIn(lines[1],again.stdout)
        self.assertEqual((self.install/'.dashboard-initial-credentials').read_bytes(),token)
        self.assertEqual((self.install/'.dashboard-initial-credentials').stat().st_mode&0o777,0o600)
        binary=self.install/'bin/stream-bridge-server'
        binary.write_text('#!/bin/bash\nprintf "%s" "${DASHBOARD_INITIAL_PASSWORD:-}"\n')
        marker=self.root/'never-run'
        token=f'$(touch "{marker}")'
        (self.install/'.dashboard-initial-credentials').write_text('sb_test01\n'+token+'\n')
        env=dict(os.environ);env.pop('DASHBOARD_INITIAL_PASSWORD',None);env.pop('DASHBOARD_INITIAL_USERNAME',None)
        result=subprocess.run([str(self.install/'run.sh'),'--check'],env=env,text=True,capture_output=True,check=True)
        self.assertEqual(result.stdout,token)
        self.assertFalse(marker.exists())
        env['DASHBOARD_INITIAL_PASSWORD']='exported-token'
        result=subprocess.run([str(self.install/'run.sh'),'--check'],env=env,text=True,capture_output=True,check=True)
        self.assertEqual(result.stdout,'exported-token')
        (self.install/'dashboard-admin.toml').write_text('saved password hash')
        (self.install/'.dashboard-initial-credentials').unlink()
        again=subprocess.run([str(script)],check=True,capture_output=True,text=True)
        self.assertNotIn('Initial dashboard password:',again.stdout)
        self.assertFalse((self.install/'.dashboard-initial-credentials').exists())

    def test_dashboard_setup_only_creates_credentials_after_assets_are_ready(self):
        result=self.install_package();self.assertEqual(result.returncode,0,result.stderr)
        script=self.install/'setup-dashboard.sh'
        failed=subprocess.run([str(script)],capture_output=True,text=True)
        self.assertNotEqual(failed.returncode,0)
        self.assertFalse((self.install/'.dashboard-initial-credentials').exists())
        assets=self.install/'dashboard/dist';assets.mkdir(parents=True)
        (assets/'index.html').write_text('<html></html>')
        env=dict(os.environ,DASHBOARD_INITIAL_USERNAME='admin')
        failed=subprocess.run([str(script)],env=env,capture_output=True,text=True)
        self.assertNotEqual(failed.returncode,0)
        self.assertFalse((self.install/'.dashboard-initial-credentials').exists())
        env['DASHBOARD_INITIAL_USERNAME']='my_bridge'
        result=subprocess.run([str(script)],env=env,check=True,capture_output=True,text=True)
        self.assertIn('Initial dashboard username: my_bridge',result.stdout)

    def test_dashboard_source_upgrade_rebuilds_existing_assets_and_keeps_credentials(self):
        checkout=self.root/'source checkout'
        assets=checkout/'dashboard/dist';assets.mkdir(parents=True)
        shutil.copy2(ROOT/'setup-dashboard.sh',checkout/'setup-dashboard.sh')
        (assets/'index.html').write_text('old token login')
        (checkout/'dashboard/package-lock.json').write_text('{}')
        credentials=checkout/'.dashboard-initial-credentials'
        credentials.write_text('sb_test01\nexisting-password\n')
        fake_bin=self.root/'fake-bin';fake_bin.mkdir()
        npm=fake_bin/'npm'
        npm.write_text('#!/bin/bash\nset -e\nprintf "%s\\n" "$*" >> npm-calls\nif [[ "$*" == "run build" ]]; then printf "new password login" > dist/index.html; fi\n')
        npm.chmod(0o755)
        env=dict(os.environ,PATH=str(fake_bin)+os.pathsep+os.environ['PATH'])
        result=subprocess.run(['bash',str(checkout/'setup-dashboard.sh')],env=env,text=True,capture_output=True)
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual((assets/'index.html').read_text(),'new password login')
        self.assertEqual((checkout/'dashboard/npm-calls').read_text().splitlines(),['ci --no-audit --no-fund','run build'])
        self.assertEqual(credentials.read_text(),'sb_test01\nexisting-password\n')
        self.assertNotIn('existing-password',result.stdout)

        # A failed source rebuild must stop before generating credentials.
        credentials.unlink()
        npm.write_text('#!/bin/bash\nexit 42\n')
        failed=subprocess.run(['bash',str(checkout/'setup-dashboard.sh')],env=env,text=True,capture_output=True)
        self.assertNotEqual(failed.returncode,0)
        self.assertFalse(credentials.exists())

        # A prebuilt release needs no npm, even if one is installed but unusable.
        (checkout/'dashboard/package-lock.json').unlink()
        result=subprocess.run(['bash',str(checkout/'setup-dashboard.sh')],env=env,text=True,capture_output=True)
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertTrue(credentials.exists())

    def test_bad_or_missing_checksum_does_not_replace_install(self):
        result=self.install_package();self.assertEqual(result.returncode,0,result.stderr)
        original=(self.install/'config.toml').read_bytes()
        self.archive.write_bytes(b'corrupt')
        self.assertNotEqual(self.install_package().returncode,0)
        self.assertEqual((self.install/'config.toml').read_bytes(),original)
        (self.assets/'SHA256SUMS').unlink()
        self.assertNotEqual(self.install_package().returncode,0)

    def test_rejects_unmanaged_and_symlink_destinations(self):
        self.install.mkdir();(self.install/'keep').write_text('keep')
        self.assertNotEqual(self.install_package().returncode,0)
        self.assertEqual((self.install/'keep').read_text(),'keep')
        self.install=self.root/'link';self.install.symlink_to(self.root/'stage')
        self.assertNotEqual(self.install_package().returncode,0)

    def test_rejects_archive_traversal_even_with_matching_checksum(self):
        with tarfile.open(self.archive,'w:gz') as tar:
            entry=tarfile.TarInfo('stream-bridge/../../escape');entry.size=1
            tar.addfile(entry,io.BytesIO(b'x'))
        self.checksum()
        self.assertNotEqual(self.install_package().returncode,0)
        self.assertFalse((self.root/'escape').exists())

    def test_receiver_setup_creates_credentials_and_keeps_them(self):
        result=self.install_package('receiver');self.assertEqual(result.returncode,0,result.stderr)
        self.assertIn('role = "receiver"',(self.install/'config.toml').read_text())
        token=(self.install/'.bridge-token').read_bytes()
        cert=(self.install/'certs/tunnel-cert.pem').read_bytes()
        self.assertGreaterEqual(len(token.strip()),32)
        self.assertEqual((self.install/'certs/tunnel-key.pem').stat().st_mode&0o777,0o600)
        result=self.install_package('receiver');self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual((self.install/'.bridge-token').read_bytes(),token)
        self.assertEqual((self.install/'certs/tunnel-cert.pem').read_bytes(),cert)

    def test_shared_receiver_template_and_credentials(self):
        result=self.install_package('shared-receiver');self.assertEqual(result.returncode,0,result.stderr)
        self.assertIn('[grpc.shared]',(self.install/'config.toml').read_text())
        self.assertTrue((self.install/'.bridge-token').is_file())
        self.assertTrue((self.install/'certs/tunnel-key.pem').is_file())

    def test_private_setup_creates_only_token_and_keeps_it_on_upgrade(self):
        result=self.install_package('private-receiver');self.assertEqual(result.returncode,0,result.stderr)
        config=(self.install/'config.toml').read_text()
        self.assertIn('insecure = true',config)
        self.assertIn('allowed_sources = ["10.0.0.20", "10.0.0.30"]',config)
        token=(self.install/'.bridge-token').read_bytes()
        self.assertGreaterEqual(len(token.strip()),32)
        self.assertEqual((self.install/'.bridge-token').stat().st_mode&0o777,0o600)
        self.assertFalse((self.install/'certs').exists())
        result=self.install_package('private-receiver');self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual((self.install/'.bridge-token').read_bytes(),token)
        self.assertEqual((self.install/'config.toml').read_text(),config)
        self.assertFalse((self.install/'certs').exists())
        self.install=self.root/'private source'
        result=self.install_package('private-source');self.assertEqual(result.returncode,0,result.stderr)
        config=(self.install/'config.toml').read_text()
        self.assertIn('insecure = true',config)
        self.assertNotIn('ca_file',config)
        self.assertFalse((self.install/'certs').exists())
        self.assertFalse((self.install/'.bridge-token').exists())

    def test_private_upgrade_keeps_existing_tls_configuration_and_credentials(self):
        result=self.install_package('receiver');self.assertEqual(result.returncode,0,result.stderr)
        config=(ROOT/'config.private-receiver.toml').read_text().replace('insecure = true','insecure = false')
        config+='\ncert_file = "certs/tunnel-cert.pem"\nkey_file = "certs/tunnel-key.pem"\n'
        (self.install/'config.toml').write_text(config)
        before={p:p.read_bytes() for p in [self.install/'.bridge-token',self.install/'certs/tunnel-cert.pem',self.install/'certs/tunnel-key.pem']}
        result=self.install_package('private-receiver');self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual((self.install/'config.toml').read_text(),config)
        for path,data in before.items():self.assertEqual(path.read_bytes(),data)

if __name__=='__main__': unittest.main()
