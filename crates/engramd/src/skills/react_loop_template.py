#!/usr/bin/env python3
"""react_loop_template — Engram skill (no network).

Generates a ReAct (Reason + Act + Observe) agent-loop SCAFFOLD for a task:
a plain-English pseudocode description of the Thought/Action/Observation
loop plus a runnable, commented Python skeleton with a placeholder LLM
call, action dispatch over the given tools, and a stopping condition.

Request (stdin): {"task": str, "tools"?: [str], "max_steps"?: int (default 8)}
Output (stdout): {task, tools, max_steps, pseudocode, python_skeleton, notes}
"""
import json
import sys

DEFAULT_TOOLS = ["search", "calculator", "code_exec"]


def build_pseudocode(task, tools, max_steps):
    tool_list = ", ".join(tools)
    return (
        "GOAL: %s\n"
        "AVAILABLE TOOLS: %s\n"
        "for step in 1..%d:\n"
        "    Thought:      reason about what to do next given the goal + past observations\n"
        "    Action:       pick ONE tool from [%s] and its input\n"
        "    Observation:  run the tool, record its result\n"
        "    if the goal is satisfied -> emit Final Answer and STOP\n"
        "if no answer after %d steps -> STOP and return best-effort answer + why it stalled"
        % (task, tool_list, max_steps, tool_list, max_steps)
    )


def build_skeleton(tools, max_steps):
    tools_literal = "[" + ", ".join('"%s"' % t for t in tools) + "]"
    return (
        "def agent_loop(task, tools=%s, max_steps=%d):\n"
        "    \"\"\"ReAct loop scaffold — replace the placeholder LLM + tool calls.\"\"\"\n"
        "    scratchpad = []  # list of (thought, action, action_input, observation)\n"
        "    for step in range(max_steps):\n"
        "        # 1. REASON: ask the LLM for the next Thought + Action given the scratchpad.\n"
        "        #    Prompt it to reply in a parseable form, e.g.:\n"
        "        #      Thought: <reasoning>\n"
        "        #      Action: <tool name from `tools`> | FINAL\n"
        "        #      Action Input: <string>\n"
        "        response = call_llm(task, tools, scratchpad)   # TODO: your LLM call\n"
        "        thought, action, action_input = parse(response)  # TODO: parse the 3 fields\n"
        "\n"
        "        # 2. STOP if the model decided it is done.\n"
        "        if action == \"FINAL\":\n"
        "            return action_input  # the final answer\n"
        "\n"
        "        # 3. ACT: dispatch to exactly one tool.\n"
        "        if action in tools:\n"
        "            observation = run_tool(action, action_input)  # TODO: execute the tool\n"
        "        else:\n"
        "            observation = \"error: unknown tool %%r; choose one of %%r\" %% (action, tools)\n"
        "\n"
        "        # 4. OBSERVE: record and (optionally) verify the result before looping.\n"
        "        scratchpad.append((thought, action, action_input, observation))\n"
        "\n"
        "    # Stopping condition: ran out of steps without a FINAL answer.\n"
        "    return \"stopped after max_steps without a final answer; scratchpad=%%r\" %% scratchpad\n"
        % (tools_literal, max_steps)
    )


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    example = {"task": "Find the population of France and double it",
               "tools": ["search", "calculator"], "max_steps": 6}

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": example}))
        return 0

    task = q.get("task")
    if not isinstance(task, str) or not task.strip():
        print(json.dumps({"error": "missing required field 'task' (string)", "example": example}))
        return 0
    task = task.strip()

    tools = q.get("tools")
    if tools is None:
        tools = list(DEFAULT_TOOLS)
    if not isinstance(tools, list):
        print(json.dumps({"error": "'tools' must be a list of strings", "example": example}))
        return 0
    tools = [str(t).strip() for t in tools if str(t).strip()]
    if not tools:
        tools = list(DEFAULT_TOOLS)

    max_steps = q.get("max_steps", 8)
    try:
        max_steps = int(max_steps)
    except (TypeError, ValueError):
        print(json.dumps({"error": "'max_steps' must be an integer", "example": example}))
        return 0
    if max_steps < 1:
        max_steps = 1
    if max_steps > 50:
        max_steps = 50

    try:
        result = {
            "task": task,
            "tools": tools,
            "max_steps": max_steps,
            "pseudocode": build_pseudocode(task, tools, max_steps),
            "python_skeleton": build_skeleton(tools, max_steps),
            "notes": [
                "SCAFFOLD: fill in call_llm(), parse() and run_tool() for your stack.",
                "Interleave reasoning and acting — one Thought, then one Action, then Observe.",
                "Cap the loop with max_steps to prevent runaway/looping agents.",
                "Verify observations before trusting them; tools can return errors or junk.",
                "Keep the scratchpad in the prompt so each Thought sees prior Observations.",
            ],
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "react_loop_template failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
