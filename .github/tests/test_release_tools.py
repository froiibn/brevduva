# Copyright 2026 SEIZIA (Jaeyoung Ko)
# SPDX-License-Identifier: Apache-2.0
import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("release_tools", Path(__file__).parents[1] / "release_tools.py")
tools = importlib.util.module_from_spec(spec)
spec.loader.exec_module(tools)
SHA = "a" * 40

class ReleaseTests(unittest.TestCase):
    def record(self, number=1, status="completed", conclusion="success", sha=SHA):
        return dict(id=number, head_sha=sha, event="push", status=status, conclusion=conclusion, html_url="test")

    def test_exact_success(self):
        with patch.object(tools, "api", return_value={"workflow_runs":[self.record()]}):
            self.assertEqual(tools.successful_run("owner/repo", SHA)["id"], 1)

    def test_no_missing_pending_failed_or_wrong_sha(self):
        for records in [[], [self.record(status="in_progress")], [self.record(conclusion="failure")], [self.record(sha="b"*40)]]:
            with patch.object(tools, "api", return_value={"workflow_runs":records}), self.assertRaises(ValueError):
                tools.successful_run("owner/repo", SHA)

    def test_old_success_cannot_hide_new_failure(self):
        with patch.object(tools, "api", return_value={"workflow_runs":[self.record(),self.record(2,conclusion="failure")]}), self.assertRaises(ValueError):
            tools.successful_run("owner/repo", SHA)

    def test_api_failure_is_not_success(self):
        with patch.object(tools, "api", side_effect=RuntimeError("offline")), self.assertRaises(RuntimeError):
            tools.successful_run("owner/repo", SHA)

    def test_all_five_nonempty_assets_required(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp)
            with self.assertRaises(ValueError): tools.validate_assets(path)
            for name in tools.ASSETS: (path/name).write_bytes(name.encode())
            self.assertEqual(len(tools.validate_assets(path).splitlines()),5)
            name = next(iter(tools.ASSETS))
            (path/name).write_bytes(b"")
            with self.assertRaises(ValueError): tools.validate_assets(path)

    def test_published_release_is_never_overwritten(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp)
            for name in tools.ASSETS: (path/name).write_bytes(b"asset")
            with patch.object(tools,"run",return_value='[[{"tag_name":"v1.0.0","draft":false}]]') as command:
                with self.assertRaises(ValueError): tools.publish("owner/repo","v1.0.0",path)
                self.assertEqual(command.call_count,1)

    def test_remote_corruption_never_promotes_draft(self):
        for corrupt in [False,True]:
            with tempfile.TemporaryDirectory() as temp:
                path=Path(temp)
                for name in tools.ASSETS: (path/name).write_bytes(name.encode())
                commands=[]
                def command(*args):
                    commands.append(args)
                    if args[:3]==("gh","release","download"):
                        remote=Path(args[args.index("--dir")+1])
                        for item in path.iterdir(): (remote/item.name).write_bytes(item.read_bytes())
                        if corrupt: (remote/next(iter(tools.ASSETS))).write_bytes(b"corrupted")
                    return ""
                with patch.object(tools,"find_release",return_value={"draft":True}),patch.object(tools,"run",side_effect=command):
                    if corrupt:
                        with self.assertRaises(ValueError): tools.publish("owner/repo","v1.0.0",path)
                    else: tools.publish("owner/repo","v1.0.0",path)
                promotions=[args for args in commands if args[:3]==("gh","release","edit")]
                self.assertEqual(len(promotions),0 if corrupt else 1)

if __name__ == "__main__": unittest.main()
