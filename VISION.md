# Engram, Vision

> *An engram is the physical trace a memory leaves in living tissue. This agent is built to leave traces, to remember, to strengthen what it uses, to forget what it doesn't, and to grow with the person it serves.*

## What this is

Engram is a self-improving personal AI agent modeled on how a brain actually works, then translated to a machine that costs almost nothing to run. It is built to be **measurably better than [Nous Hermes](https://github.com/nousresearch/hermes-agent)**: faster, lighter, more secure, and genuinely learning rather than merely configured.

It is not a chatbot with a database bolted on. It is a small organism: neurons that fire on events, memory that lives in different regions and is recalled by the *kind* of experience at hand, and skills that are not text prompts but **small programs that improve themselves with use**: and can grow into agents, and band together into swarms.

## First principles

1. **Mimic the brain, then make it cheap.** Hot/warm/cold memory, consolidation during rest, recall by task type, salience-weighted forgetting. Every neuroscience idea here exists because it earns its keep in capability-per-watt, not as decoration.
2. **Skills are programs, not prompts.** A skill is executable, versioned, sandboxed, and *self-improving*: it measures its own success and rewrites itself toward it. The best language is chosen per skill (Rust, Python, shell, whatever fits). A skill can graduate into a full agent; several skills can form a swarm.
3. **Near-zero idle.** A Rust core means a single small binary, tiny resident memory, and instant wake. It sleeps to nothing on a $5 VPS or a serverless trigger and wakes on an event. You should be able to forget it's running.
4. **It grows with you.** Across sessions it builds a deepening, auditable model of who you are, your projects, your style, your preferences, and gets more useful the longer you use it.
5. **Transparent and auditable.** Every memory write, every skill mutation, every autonomous action is logged, attributable, and reversible. You can watch the brain think.
6. **Less is more.** The smallest design that delivers the vision wins. Capability comes from architecture and the right language, not from lines of code.
7. **Reactive and proactive.** Neurons react to events; the agent proactively proposes improvements, to itself, to its skills, and to your work.

## The brain, mapped to the machine

| Brain | Engram | Role |
|---|---|---|
| Working memory (prefrontal) | **Hot** store, in-RAM | the current task and context |
| Hippocampus | **Episodic** encoder + consolidator | recent experiences, conversations |
| Neocortex | **Semantic** store (warm→cold) | consolidated long-term knowledge |
| Basal ganglia | **Procedural** store | skills / habits, the self-improving programs |
| Amygdala | **Salience** tagging | what matters, what to keep, what to forget |
| Neurons / synapses | **Reactive event bus** | fire on events; synaptic weights = learning |
| Sleep / consolidation | **Idle consolidation** | move warm→cold, strengthen used traces, prune the rest |
| Self-model | **Identity model** | the deepening picture of *you* |

## How we beat Hermes

The architecture workflow sets concrete targets, but the thesis is constant:
- **Footprint:** a Rust core idles in megabytes, not the hundreds a Python runtime needs - so it actually costs nothing idle.
- **Speed:** cold-start in milliseconds; recall that's indexed and tiered, not a linear scan.
- **Security:** self-modifying code is the scariest attack surface there is, so skills run sandboxed under an explicit capability model, and every mutation is signed and reversible.
- **Learning:** real procedural learning - skills that rewrite themselves from measured outcomes - not a static skill library.
- **Presence:** a desktop interface that makes the brain *visible* - memory tiers, firing neurons, skills evolving, the model of you deepening.

## Non-negotiables

- Runs on a cheap VPS and serverless; ~zero cost idle.
- Polyglot skills; reactive core.
- Everything transparent, auditable, reversible.
- Authored by Radoslav Tsvetkov.

---
*This document states the destination. The architecture decision record (`docs/ADR-0001-architecture.md`) states how we get there; the code proves it.*
