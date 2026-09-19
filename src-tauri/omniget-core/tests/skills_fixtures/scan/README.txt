Reports in the shape NVIDIA SkillSpector writes with `scan --no-llm --format json`.

Ported by hand on 18/09/2026 from the source of the tool itself, not from its
README alone:

  src/skillspector/nodes/report.py::_format_json   the top-level object
  src/skillspector/models.py::Finding.to_dict      every field of issues[]
  src/skillspector/nodes/report.py::_build_metadata  the metadata block
  src/skillspector/constants.py::RISK_THRESHOLD    50, and score > 50 is unsafe

clean.json  a skill with no findings: score 0, LOW, SAFE. The CLI exits 0.
risky.json  a skill with a CRITICAL supply-chain finding: score 85, CRITICAL,
            DO_NOT_INSTALL. The CLI exits 1 on this one — a completed scan over
            the threshold, which is a verdict and not a failure.

SkillSpector is Apache-2.0. These are fixtures in its output shape, not copies
of its code.
