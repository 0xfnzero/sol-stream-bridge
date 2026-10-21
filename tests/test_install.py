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
        for name in ['setup.sh','run.sh']:
            shutil.copy2(ROOT/name,package/name)
        for name in ['config.toml','config.receiver.toml','config.shred-only.toml','config.private-source.toml','config.private-receiver.toml']:
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
        (self.install/'certs').mkdir();(self.install/'certs/private.pem').write_text('test certificate')
        result=self.install_package();self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual((self.install/'config.toml').read_text(),'personal config\n')
        self.assertTrue((self.install/'custom.toml').exists())
        self.assertTrue((self.install/'certs/private.pem').exists())
        self.assertEqual((self.install/'.bridge-token').stat().st_mode&0o777,0o600)
        env=dict(os.environ);env.pop('BRIDGE_TOKEN',None)
        result=subprocess.run([str(self.install/'run.sh'),'--check'],env=env,text=True,capture_output=True,check=True)
        self.assertEqual(result.stdout.strip(),token)
        self.assertFalse(marker.exists())
        self.assertEqual(len(list(self.root.glob('installed bridge.backup.*'))),1)

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

if __name__=='__main__': unittest.main()
