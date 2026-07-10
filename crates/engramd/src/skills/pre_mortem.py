#!/usr/bin/env python3
"""pre_mortem — Engram skill (no network).

Generates a pre-mortem exercise SCAFFOLD: imagine the project has FAILED six
months from now, then surface why. Produces probing failure-mode prompts
across standard categories, maps any known risks to a category, and gives a
likelihood/impact/mitigation template for each. Prompts are scaffolding to
work through as a team, not conclusions.

Request (stdin): {"project": str, "known_risks"?: [str]}
Output (stdout): {project, framing, failure_mode_prompts, mapped_known_risks,
  mitigation_template, note}
"""
import json
import sys

# (category, probing failure-mode question) — {project} is filled in.
CATEGORIES = [
    ("scope/timeline",
     "It's 6 months later and '{project}' failed on scope or schedule — what slipped, and "
     "when did we first know it would?"),
    ("technical",
     "What technical assumption, dependency or piece of complexity in '{project}' turned out "
     "to be wrong or far harder than expected?"),
    ("team/resourcing",
     "What about the team, ownership or capacity caused '{project}' to stall (key person left, "
     "unclear owner, overload)?"),
    ("market/user",
     "Why did users or the market not respond to '{project}' the way we assumed?"),
    ("external/dependencies",
     "What external factor or dependency (vendor, partner, regulation, cost) broke '{project}'?"),
]

KEYWORDS = {
    "scope/timeline": ["scope", "timeline", "schedule", "deadline", "delay", "late", "creep"],
    "technical": ["tech", "bug", "architecture", "scal", "perf", "integration", "data", "api",
                  "infra", "security"],
    "team/resourcing": ["team", "hire", "resource", "capacity", "owner", "staff", "people",
                        "burnout", "bandwidth", "engineer", "developer", "only one", "single "
                        "person", "bus factor", "knows the"],
    "market/user": ["user", "market", "customer", "adoption", "demand", "competitor", "revenue",
                    "churn"],
    "external/dependencies": ["vendor", "partner", "regulat", "legal", "cost", "budget", "supply",
                              "depend", "third"],
}


def categorize(risk):
    low = risk.lower()
    best = None
    for cat, kws in KEYWORDS.items():
        for kw in kws:
            if kw in low:
                return cat
    return best or "uncategorized"


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    example = {"project": "Migrate billing to the new provider",
               "known_risks": ["Vendor API may be rate-limited", "Only one engineer knows the code"]}

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": example}))
        return 0

    project = q.get("project")
    if not isinstance(project, str) or not project.strip():
        print(json.dumps({"error": "missing required field 'project' (string)", "example": example}))
        return 0
    project = project.strip()

    known_risks = q.get("known_risks", [])
    if known_risks is None:
        known_risks = []
    if not isinstance(known_risks, list):
        print(json.dumps({"error": "'known_risks' must be a list of strings", "example": example}))
        return 0
    known_risks = [str(r).strip() for r in known_risks if str(r).strip()]

    try:
        failure_mode_prompts = [
            {"category": cat, "prompt": prompt_t.format(project=project)}
            for cat, prompt_t in CATEGORIES
        ]

        mapped = []
        mitigation_template = []
        for r in known_risks:
            cat = categorize(r)
            mapped.append({"risk": r, "category": cat})
            mitigation_template.append({
                "risk": r,
                "category": cat,
                "likelihood_prompt": "How likely is '%s' (low / medium / high) and why?" % r,
                "impact_prompt": "If '%s' happens, how bad is the impact on '%s'?" % (r, project),
                "mitigation_prompt": "What is the single best action to prevent or reduce '%s', "
                                     "and who owns it?" % r,
            })

        result = {
            "project": project,
            "framing": ("Assume it is 6 months from now and '%s' has clearly FAILED. Working "
                        "backwards from that failure, write down every plausible reason it "
                        "happened." % project),
            "failure_mode_prompts": failure_mode_prompts,
            "mapped_known_risks": mapped,
            "mitigation_template": mitigation_template,
            "note": ("SCAFFOLD: run this as a team exercise. Have everyone independently list "
                     "failure causes BEFORE discussing (prospective hindsight surfaces risks "
                     "people won't raise in a normal status check). Then cluster, rate "
                     "likelihood x impact, and assign an owner to the top mitigations."),
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "pre_mortem failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
