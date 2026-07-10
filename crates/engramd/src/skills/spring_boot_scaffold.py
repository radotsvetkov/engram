#!/usr/bin/env python3
"""spring_boot_scaffold — Engram skill (no network). Scaffold a Spring Boot
@RestController mapped to /api/{plural} with @GetMapping/@PostMapping/
@PutMapping/@DeleteMapping methods returning ResponseEntity<>, a
constructor-injected service field, and optionally a matching @Entity/record.

Request (stdin): {"name": "product", "base_package": "com.example", "model": "Product"}
Output (stdout): {files, notes, next_steps}
"""
import json
import re
import sys


def _to_pascal_case(name):
    parts = re.split(r"[^A-Za-z0-9]+", name.strip())
    words = []
    for part in parts:
        if not part:
            continue
        sub = re.findall(r"[A-Z]+(?=[A-Z][a-z0-9])|[A-Z]?[a-z0-9]+|[A-Z]+", part)
        words.extend(sub if sub else [part])
    if not words:
        return ""
    return "".join(w[:1].upper() + w[1:] for w in words if w)


def _camel(pascal):
    return pascal[:1].lower() + pascal[1:] if pascal else pascal


def _pluralize(word):
    if not word:
        return word
    lower = word.lower()
    if lower.endswith(("s", "x", "z", "ch", "sh")):
        return word + "es"
    if lower.endswith("y") and len(word) > 1 and word[-2].lower() not in "aeiou":
        return word[:-1] + "ies"
    return word + "s"


def _valid_package(pkg):
    return bool(re.match(r"^[a-z_][a-z0-9_]*(\.[a-z_][a-z0-9_]*)*$", pkg))


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"name": "Product", "base_package": "com.example", "model": "Product"},
        }))
        return 0

    raw_name = q.get("name")
    if not isinstance(raw_name, str) or not raw_name.strip():
        print(json.dumps({
            "error": "missing required field 'name' (non-empty string)",
            "example": {"name": "Product", "base_package": "com.example", "model": "Product"},
        }))
        return 0

    base_package = q.get("base_package", "com.example")
    if not isinstance(base_package, str) or not base_package.strip():
        base_package = "com.example"
    base_package = base_package.strip()
    if not _valid_package(base_package):
        print(json.dumps({
            "error": "'base_package' is not a valid Java package: %r" % base_package,
            "example": {"name": "Product", "base_package": "com.example"},
        }))
        return 0

    raw_model = q.get("model")
    model = _to_pascal_case(raw_model) if isinstance(raw_model, str) and raw_model.strip() else None

    try:
        base = _to_pascal_case(raw_name)
        if not base:
            print(json.dumps({"error": "could not derive a valid name from %r" % raw_name}))
            return 0
        if base.endswith("Controller"):
            base = base[: -len("Controller")] or base
        entity = model or base
        entity_var = _camel(entity)
        plural = _pluralize(base).lower()
        service_type = "%sService" % base
        service_var = _camel(service_type)
        pkg_path = base_package.replace(".", "/")

        L = []
        L.append("package %s.controller;" % base_package)
        L.append("")
        L.append("import java.util.List;")
        L.append("")
        L.append("import org.springframework.http.HttpStatus;")
        L.append("import org.springframework.http.ResponseEntity;")
        L.append("import org.springframework.web.bind.annotation.DeleteMapping;")
        L.append("import org.springframework.web.bind.annotation.GetMapping;")
        L.append("import org.springframework.web.bind.annotation.PathVariable;")
        L.append("import org.springframework.web.bind.annotation.PostMapping;")
        L.append("import org.springframework.web.bind.annotation.PutMapping;")
        L.append("import org.springframework.web.bind.annotation.RequestBody;")
        L.append("import org.springframework.web.bind.annotation.RequestMapping;")
        L.append("import org.springframework.web.bind.annotation.RestController;")
        L.append("")
        L.append("@RestController")
        L.append("@RequestMapping(\"/api/%s\")" % plural)
        L.append("public class %sController {" % base)
        L.append("")
        L.append("    private final %s %s;" % (service_type, service_var))
        L.append("")
        L.append("    public %sController(%s %s) {" % (base, service_type, service_var))
        L.append("        this.%s = %s;" % (service_var, service_var))
        L.append("    }")
        L.append("")
        L.append("    @GetMapping")
        L.append("    public ResponseEntity<List<%s>> findAll() {" % entity)
        L.append("        return ResponseEntity.ok(%s.findAll());" % service_var)
        L.append("    }")
        L.append("")
        L.append("    @GetMapping(\"/{id}\")")
        L.append("    public ResponseEntity<%s> findById(@PathVariable Long id) {" % entity)
        L.append("        return %s.findById(id)" % service_var)
        L.append("                .map(ResponseEntity::ok)")
        L.append("                .orElseGet(() -> ResponseEntity.notFound().build());")
        L.append("    }")
        L.append("")
        L.append("    @PostMapping")
        L.append("    public ResponseEntity<%s> create(@RequestBody %s %s) {" % (entity, entity, entity_var))
        L.append("        %s created = %s.save(%s);" % (entity, service_var, entity_var))
        L.append("        return ResponseEntity.status(HttpStatus.CREATED).body(created);")
        L.append("    }")
        L.append("")
        L.append("    @PutMapping(\"/{id}\")")
        L.append("    public ResponseEntity<%s> update(@PathVariable Long id, @RequestBody %s %s) {" % (entity, entity, entity_var))
        L.append("        return ResponseEntity.ok(%s.update(id, %s));" % (service_var, entity_var))
        L.append("    }")
        L.append("")
        L.append("    @DeleteMapping(\"/{id}\")")
        L.append("    public ResponseEntity<Void> delete(@PathVariable Long id) {")
        L.append("        %s.deleteById(id);" % service_var)
        L.append("        return ResponseEntity.noContent().build();")
        L.append("    }")
        L.append("}")
        L.append("")
        controller_code = "\n".join(L)

        files = {
            "src/main/java/%s/controller/%sController.java" % (pkg_path, base): controller_code,
        }

        notes = [
            "@RestController mapped to /api/%s with GET/GET{id}/POST/PUT/DELETE returning ResponseEntity<>." % plural,
            "Constructor-injected %s field (no field @Autowired — the modern idiom)." % service_type,
            "References a %s you still need to implement (findAll/findById/save/update/deleteById)." % service_type,
        ]

        if model:
            eL = []
            eL.append("package %s.model;" % base_package)
            eL.append("")
            eL.append("import jakarta.persistence.Entity;")
            eL.append("import jakarta.persistence.GeneratedValue;")
            eL.append("import jakarta.persistence.GenerationType;")
            eL.append("import jakarta.persistence.Id;")
            eL.append("")
            eL.append("@Entity")
            eL.append("public class %s {" % model)
            eL.append("")
            eL.append("    @Id")
            eL.append("    @GeneratedValue(strategy = GenerationType.IDENTITY)")
            eL.append("    private Long id;")
            eL.append("")
            eL.append("    // TODO: add the remaining fields.")
            eL.append("    private String name;")
            eL.append("")
            eL.append("    public Long getId() {")
            eL.append("        return id;")
            eL.append("    }")
            eL.append("")
            eL.append("    public void setId(Long id) {")
            eL.append("        this.id = id;")
            eL.append("    }")
            eL.append("")
            eL.append("    public String getName() {")
            eL.append("        return name;")
            eL.append("    }")
            eL.append("")
            eL.append("    public void setName(String name) {")
            eL.append("        this.name = name;")
            eL.append("    }")
            eL.append("}")
            eL.append("")
            files["src/main/java/%s/model/%s.java" % (pkg_path, model)] = "\n".join(eL)
            notes.append("Includes a JPA @Entity %s (jakarta.persistence, Spring Boot 3+)." % model)

        next_steps = [
            "Create the %s (annotate with @Service) exposing findAll/findById/save/update/deleteById." % service_type,
            "Add a Spring Data repository (e.g. `interface %sRepository extends JpaRepository<%s, Long>`)." % (entity, entity),
        ]

        result = {"files": files, "notes": notes, "next_steps": next_steps}
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "spring_boot_scaffold failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
