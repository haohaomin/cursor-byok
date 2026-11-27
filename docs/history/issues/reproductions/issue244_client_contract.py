"""Read a Cursor 3.31.15 artifact; evaluate one pure method with fake state.

This does not launch Cursor or demonstrate UI, network, or agent execution.
Run with Python 3 and Node.js; pass the Resources/app directory as argv[1].
"""
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys


root = Path(sys.argv[1])
expected = {
    "extensions/cursor-local-agent-runtime/dist/main.js":
        "a637fafae17faca9c0d07b6fac0a31c1cb63915335a78527e004567142cd834c",
    "extensions/cursor-agent-host/dist/main.js":
        "c7170dacc639cfd42a671527fb40175594096f6db53c2a96fe738b85f7121528",
    "out/vs/workbench/workbench.desktop.main.js":
        "79a1c7cc4fc3b14df093b18829ab88c1ee955e60ea49bb7984dbaf488f033713",
}
record = {
    "cursor_version": json.loads((root / "package.json").read_text())["version"],
    "scope": "Shipped-file inspection and isolated extracted-method probe; not live Cursor execution",
    "rea_parent_evidence": "ev_a485fa596bb60a5bf97cc88b22f0197ca1c4331db3c49acfa96d3cc6ff31806b",
    "artifacts": {},
}
assert record["cursor_version"] == "3.31.15"
texts = {}
for name, digest in expected.items():
    data = (root / name).read_bytes()
    assert hashlib.sha256(data).hexdigest() == digest, name
    text = data.decode()
    texts[name] = text
    item = {
        "sha256": digest,
        "bytes": len(data),
        "legacy_tool_literal_counts": {
            term: text.count(term) for term in ["create-agent", "send-message-to-agent"]
        },
    }
    if "workbench" not in name:
        match = re.search(r'"AgentMode",(\[\[.*?\]\]),1\)', text)
        assert match, name
        modes = json.loads(match[1])
        assert all(number != 6 and label != "PROJECT" for number, label in modes)
        item["agent_mode_definition_character_offset"] = match.start()
        item["agent_modes"] = modes
    record["artifacts"][name] = item

workbench = texts["out/vs/workbench/workbench.desktop.main.js"]
start = workbench.index("forceBackgroundByToolCallId(e){")
end = workbench.index("async handoffToBackground", start)
method = workbench[start:end]
assert "activeForegroundExecutions.get" in method
assert "pendingForegroundBackgroundToolCallIds.add" in method
assert "requestBackground()" in method

# Evaluate only the extracted method, with fake Maps, status values and result class.
# No module bootstrap, filesystem, network, subprocess or real client state is exposed.
probe = r"""
const vm = require('node:vm');
let source = '';
process.stdin.setEncoding('utf8');
process.stdin.on('data', part => source += part);
process.stdin.on('end', () => {
  const sourceLiteral = JSON.stringify(source);
  const result = vm.runInNewContext(`
    const oIt = {ACCEPTED: 1, NOT_FOUND: 2};
    const lKp = 100;
    class rlr { constructor(value) { this.status = value.status; } }
    const handler = ({${JSON.parse(sourceLiteral)}}).forceBackgroundByToolCallId;
    let backgroundRequests = 0;
    const state = {
      activeForegroundExecutions: new Map(),
      pendingForegroundBackgroundToolCallIds: new Set()
    };
    const absent = handler.call(state, 'new-tool-call');
    const afterAbsent = {
      status: absent.status,
      activeExecutions: state.activeForegroundExecutions.size,
      pendingIds: [...state.pendingForegroundBackgroundToolCallIds],
      backgroundRequests
    };
    state.activeForegroundExecutions.set('existing-call', {
      requestBackground: () => { backgroundRequests++; return true; }
    });
    const existing = handler.call(state, 'existing-call');
    JSON.stringify({absent: afterAbsent, existing: {
      status: existing.status, backgroundRequests,
      activeExecutions: state.activeForegroundExecutions.size
    }});
  `, {}, {timeout: 1000});
  process.stdout.write(result);
});
"""
result = json.loads(subprocess.check_output(["node", "-e", probe], input=method, text=True))
assert result["absent"] == {
    "status": 1, "activeExecutions": 0,
    "pendingIds": ["new-tool-call"], "backgroundRequests": 0,
}
assert result["existing"] == {"status": 1, "backgroundRequests": 1, "activeExecutions": 1}
record["force_background_probe"] = {
    "method_character_range": [start, end],
    "method_sha256": hashlib.sha256(method.encode()).hexdigest(),
    "result": result,
    "limitation": "Extracted method with fake Maps and enum/result stubs; not a live agent creation test",
}
print(json.dumps(record, ensure_ascii=False, indent=2))
