import os
from pathlib import Path
import runpy
import struct
import tempfile
import unittest
import uuid
import zipfile


RELEASE = runpy.run_path(str(Path(__file__).resolve().parents[1] / "release-macos.py"))


class ReleaseTests(unittest.TestCase):
    def executable(self, root, identifier, payload=b"native code"):
        path = Path(root) / "Dzban"
        header = struct.pack("<8I", 0xFEEDFACF, 0x0100000C, 0, 2, 1, 24, 0, 0)
        path.write_bytes(header + struct.pack("<II", 0x1B, 24) + identifier + payload)
        return path

    def test_relinked_uuid_normalizes_to_identical_executable_bytes(self):
        with tempfile.TemporaryDirectory() as first, tempfile.TemporaryDirectory() as second:
            a = self.executable(first, bytes([17]) * 16)
            b = self.executable(second, bytes([170]) * 16)
            RELEASE["normalize_uuid"](a)
            RELEASE["normalize_uuid"](b)
            self.assertEqual(a.read_bytes(), b.read_bytes())
            self.assertEqual(uuid.UUID(bytes=a.read_bytes()[40:56]).version, 5)

    def test_changed_native_code_changes_the_uuid(self):
        with tempfile.TemporaryDirectory() as first, tempfile.TemporaryDirectory() as second:
            a = self.executable(first, bytes(16), b"first code")
            b = self.executable(second, bytes(16), b"second code")
            RELEASE["normalize_uuid"](a)
            RELEASE["normalize_uuid"](b)
            self.assertNotEqual(a.read_bytes()[40:56], b.read_bytes()[40:56])

    def test_missing_loader_uuid_is_rejected_without_changing_the_file(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "Dzban"
            original = struct.pack("<8I", 0xFEEDFACF, 0x0100000C, 0, 2, 0, 0, 0, 0)
            path.write_bytes(original)
            with self.assertRaisesRegex(SystemExit, "requires an LC_UUID"):
                RELEASE["normalize_uuid"](path)
            self.assertEqual(path.read_bytes(), original)

    def test_archive_ignores_build_paths_and_file_times_and_preserves_executable_mode(self):
        with tempfile.TemporaryDirectory() as directory:
            archives = []
            for index in range(2):
                root = Path(directory) / str(index)
                app = root / "Dzban.app"
                executable = app / "Contents/MacOS/Dzban"
                executable.parent.mkdir(parents=True)
                executable.write_bytes(b"native executable")
                executable.chmod(0o755)
                os.utime(executable, (100 + index, 100 + index))
                (app / "Shortcut").symlink_to("Contents/MacOS/Dzban")
                destination = root / "Dzban.zip"
                RELEASE["package"](app, destination, 946684800)
                archives.append(destination)
            self.assertEqual(archives[0].read_bytes(), archives[1].read_bytes())
            with zipfile.ZipFile(archives[0]) as archive:
                self.assertTrue(archive.getinfo("Dzban.app/Contents/MacOS/Dzban").external_attr >> 16 & 0o100)
                self.assertEqual(archive.read("Dzban.app/Shortcut"), b"Contents/MacOS/Dzban")


if __name__ == "__main__":
    unittest.main()
