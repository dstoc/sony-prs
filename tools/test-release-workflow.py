#!/usr/bin/env python3
"""Check the release component and its artifact-publication workflow."""

import json
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
COMPONENT_PATH = "crates/prs-t1-agent"
RELEASE_OUTPUT = "crates/prs-t1-agent--release_created"
TAG_OUTPUT = "crates/prs-t1-agent--tag_name"


def main() -> None:
    config = json.loads((ROOT / "release-please-config.json").read_text())
    component = config["packages"][COMPONENT_PATH]
    assert component["release-type"] == "rust"
    assert component["component"] == "prs-t1-agent"
    assert component["include-component-in-tag"] is True

    manifest = json.loads((ROOT / ".release-please-manifest.json").read_text())
    assert manifest == {COMPONENT_PATH: "0.1.0"}

    workflow = (ROOT / ".github/workflows/release-please.yml").read_text()
    assert "googleapis/release-please-action@a02a34c4d625f9be7cb89156071d8567266a2445" in workflow
    assert "config-file: release-please-config.json" in workflow
    assert "manifest-file: .release-please-manifest.json" in workflow
    assert f"steps.release.outputs['{RELEASE_OUTPUT}'] == 'true'" in workflow
    assert f"steps.release.outputs['{TAG_OUTPUT}']" in workflow
    assert "RUST_TOOLCHAIN: \"1.88.0\"" in workflow
    assert "version: \"0.13.0\"" in workflow
    assert 'CARGO_ZIGBUILD_VERSION: "0.23.4"' in workflow
    assert "--target \"$TARGET\"" in workflow
    assert "readelf -h" in workflow
    assert "readelf -l" in workflow
    assert "readelf -A" in workflow
    assert "Version5 EABI, soft-float ABI" in workflow
    assert "gh release upload" in workflow
    assert "dist/prs-t1-agent-armv5te" in workflow

    print("release configuration and publication workflow checks passed")


if __name__ == "__main__":
    main()
