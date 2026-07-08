#!/usr/bin/env python3
"""p5js_sketch_scaffold — Engram skill (no network). Generate ready-to-run
p5.js sketch boilerplate for common creative-coding patterns.

Supported sketch_type values: particle_system, animation_loop,
interactive_canvas, basic_shapes. Each produces syntactically valid
JavaScript with a setup()/draw() pair sized to the requested canvas.

Request (stdin): {"sketch_type": "particle_system", "canvas_width": 800, "canvas_height": 600}
Output (stdout): {filename: "sketch.js", code: str, canvas_width, canvas_height}
"""
import json
import sys

SUPPORTED = ("particle_system", "animation_loop", "interactive_canvas", "basic_shapes")


def _particle_system(w, h):
    return """class Particle {
  constructor(x, y) {
    this.pos = createVector(x, y);
    this.vel = createVector(random(-1, 1), random(-2, -0.5));
    this.lifespan = 255;
  }

  update() {
    this.pos.add(this.vel);
    this.lifespan -= 4;
  }

  isDead() {
    return this.lifespan <= 0;
  }

  display() {
    noStroke();
    fill(200, 100, 255, this.lifespan);
    circle(this.pos.x, this.pos.y, 12);
  }
}

let particles = [];

function setup() {
  createCanvas(%d, %d);
}

function draw() {
  background(20);

  // Spawn a new particle near the mouse (or canvas center) each frame.
  particles.push(new Particle(mouseX || width / 2, mouseY || height / 2));

  for (let i = particles.length - 1; i >= 0; i--) {
    const p = particles[i];
    p.update();
    p.display();
    if (p.isDead()) {
      particles.splice(i, 1);
    }
  }
}
""" % (w, h)


def _animation_loop(w, h):
    return """function setup() {
  createCanvas(%d, %d);
}

function draw() {
  background(30);

  // Oscillate a circle up and down using frameCount as the animation clock.
  const y = height / 2 + sin(frameCount * 0.05) * (height / 4);
  noStroke();
  fill(100, 200, 255);
  circle(width / 2, y, 60);
}
""" % (w, h)


def _interactive_canvas(w, h):
    return """let drawnPoints = [];

function setup() {
  createCanvas(%d, %d);
  background(240);
}

function mousePressed() {
  drawnPoints.push({ x: mouseX, y: mouseY });
}

function mouseDragged() {
  drawnPoints.push({ x: mouseX, y: mouseY });
}

function draw() {
  background(240);
  noStroke();
  fill(50);
  for (const p of drawnPoints) {
    circle(p.x, p.y, 8);
  }
  // Live cursor indicator that follows the mouse.
  noFill();
  stroke(150);
  circle(mouseX, mouseY, 20);
}
""" % (w, h)


def _basic_shapes(w, h):
    return """function setup() {
  createCanvas(%d, %d);
}

function draw() {
  background(250);

  // Rectangle
  fill(255, 100, 100);
  rect(50, 50, 120, 80);

  // Ellipse
  fill(100, 255, 100);
  ellipse(300, 90, 100, 100);

  // Line
  stroke(0);
  strokeWeight(3);
  line(50, 200, 250, 260);

  // Triangle
  noStroke();
  fill(100, 150, 255);
  triangle(350, 250, 420, 150, 490, 250);
}
""" % (w, h)


BUILDERS = {
    "particle_system": _particle_system,
    "animation_loop": _animation_loop,
    "interactive_canvas": _interactive_canvas,
    "basic_shapes": _basic_shapes,
}


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object",
                          "example": {"sketch_type": "particle_system"}}))
        return 0

    sketch_type = q.get("sketch_type")
    if sketch_type not in SUPPORTED:
        print(json.dumps({
            "error": "'sketch_type' must be one of: %s" % ", ".join(SUPPORTED),
            "example": {"sketch_type": "particle_system"},
        }))
        return 0

    width = q.get("canvas_width", 800)
    height = q.get("canvas_height", 600)
    try:
        width = int(width)
        height = int(height)
        if width <= 0 or height <= 0:
            raise ValueError("canvas dimensions must be positive")
    except (TypeError, ValueError) as e:
        print(json.dumps({"error": "'canvas_width'/'canvas_height' must be positive integers: %s" % e}))
        return 0

    try:
        code = BUILDERS[sketch_type](width, height)
    except Exception as e:
        print(json.dumps({"error": "sketch generation failed: %s" % e}))
        return 1

    print(json.dumps({
        "filename": "sketch.js",
        "code": code,
        "canvas_width": width,
        "canvas_height": height,
    }, indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())
