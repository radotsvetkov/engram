#!/usr/bin/env python3
"""nestjs_scaffold — Engram skill (no network). Scaffold the standard NestJS
trio for a resource: {name}.module.ts (@Module), {name}.controller.ts
(@Controller with @Get/@Post/@Put/@Delete + route params), and
{name}.service.ts (@Injectable with in-memory CRUD stubs), wired together.

Request (stdin): {"name": "product", "typescript": true}
Output (stdout): {files, notes, next_steps}
"""
import json
import re
import sys


def _split_words(name):
    parts = re.split(r"[^A-Za-z0-9]+", name.strip())
    words = []
    for part in parts:
        if not part:
            continue
        sub = re.findall(r"[A-Z]+(?=[A-Z][a-z0-9])|[A-Z]?[a-z0-9]+|[A-Z]+", part)
        words.extend(sub if sub else [part])
    return [w for w in words if w]


def _to_pascal_case(name):
    return "".join(w[:1].upper() + w[1:] for w in _split_words(name))


def _to_kebab(name):
    return "-".join(w.lower() for w in _split_words(name))


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


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"name": "Product", "typescript": True},
        }))
        return 0

    raw_name = q.get("name")
    if not isinstance(raw_name, str) or not raw_name.strip():
        print(json.dumps({
            "error": "missing required field 'name' (non-empty string)",
            "example": {"name": "Product", "typescript": True},
        }))
        return 0

    # NestJS is TypeScript-first; default true.
    typescript = bool(q.get("typescript", True))

    try:
        pascal = _to_pascal_case(raw_name)
        if not pascal:
            print(json.dumps({"error": "could not derive a valid name from %r" % raw_name}))
            return 0
        kebab = _to_kebab(raw_name)          # product / user-profile
        camel = _camel(pascal)               # product
        plural_camel = _pluralize(camel)     # products
        route = _pluralize(kebab)            # products / user-profiles

        ctrl = "%sController" % pascal
        svc = "%sService" % pascal
        module = "%sModule" % pascal
        ext = "ts" if typescript else "js"

        # ---- service ----
        s = []
        s.append("import { Injectable, NotFoundException } from '@nestjs/common';")
        s.append("")
        s.append("@Injectable()")
        s.append("export class %s {" % svc)
        s.append("  private readonly %s: any[] = [];" % plural_camel)
        s.append("  private nextId = 1;")
        s.append("")
        s.append("  findAll() {")
        s.append("    return this.%s;" % plural_camel)
        s.append("  }")
        s.append("")
        s.append("  findOne(id: number) {")
        s.append("    const found = this.%s.find((item) => item.id === id);" % plural_camel)
        s.append("    if (!found) {")
        s.append("      throw new NotFoundException(`%s #${id} not found`);" % pascal)
        s.append("    }")
        s.append("    return found;")
        s.append("  }")
        s.append("")
        s.append("  create(dto: any) {")
        s.append("    const created = { id: this.nextId++, ...dto };")
        s.append("    this.%s.push(created);" % plural_camel)
        s.append("    return created;")
        s.append("  }")
        s.append("")
        s.append("  update(id: number, dto: any) {")
        s.append("    const found = this.findOne(id);")
        s.append("    Object.assign(found, dto);")
        s.append("    return found;")
        s.append("  }")
        s.append("")
        s.append("  remove(id: number) {")
        s.append("    const index = this.%s.findIndex((item) => item.id === id);" % plural_camel)
        s.append("    if (index === -1) {")
        s.append("      throw new NotFoundException(`%s #${id} not found`);" % pascal)
        s.append("    }")
        s.append("    const [removed] = this.%s.splice(index, 1);" % plural_camel)
        s.append("    return removed;")
        s.append("  }")
        s.append("}")
        s.append("")
        service_code = "\n".join(s)

        # ---- controller ----
        c = []
        c.append("import {")
        c.append("  Body,")
        c.append("  Controller,")
        c.append("  Delete,")
        c.append("  Get,")
        c.append("  Param,")
        c.append("  ParseIntPipe,")
        c.append("  Post,")
        c.append("  Put,")
        c.append("} from '@nestjs/common';")
        c.append("import { %s } from './%s.service';" % (svc, kebab))
        c.append("")
        c.append("@Controller('%s')" % route)
        c.append("export class %s {" % ctrl)
        c.append("  constructor(private readonly %s: %s) {}" % (camel + "Service", svc))
        c.append("")
        c.append("  @Get()")
        c.append("  findAll() {")
        c.append("    return this.%s.findAll();" % (camel + "Service"))
        c.append("  }")
        c.append("")
        c.append("  @Get(':id')")
        c.append("  findOne(@Param('id', ParseIntPipe) id: number) {")
        c.append("    return this.%s.findOne(id);" % (camel + "Service"))
        c.append("  }")
        c.append("")
        c.append("  @Post()")
        c.append("  create(@Body() dto: any) {")
        c.append("    return this.%s.create(dto);" % (camel + "Service"))
        c.append("  }")
        c.append("")
        c.append("  @Put(':id')")
        c.append("  update(@Param('id', ParseIntPipe) id: number, @Body() dto: any) {")
        c.append("    return this.%s.update(id, dto);" % (camel + "Service"))
        c.append("  }")
        c.append("")
        c.append("  @Delete(':id')")
        c.append("  remove(@Param('id', ParseIntPipe) id: number) {")
        c.append("    return this.%s.remove(id);" % (camel + "Service"))
        c.append("  }")
        c.append("}")
        c.append("")
        controller_code = "\n".join(c)

        # ---- module ----
        m = []
        m.append("import { Module } from '@nestjs/common';")
        m.append("import { %s } from './%s.controller';" % (ctrl, kebab))
        m.append("import { %s } from './%s.service';" % (svc, kebab))
        m.append("")
        m.append("@Module({")
        m.append("  controllers: [%s]," % ctrl)
        m.append("  providers: [%s]," % svc)
        m.append("  exports: [%s]," % svc)
        m.append("})")
        m.append("export class %s {}" % module)
        m.append("")
        module_code = "\n".join(m)

        files = {
            "%s.module.%s" % (kebab, ext): module_code,
            "%s.controller.%s" % (kebab, ext): controller_code,
            "%s.service.%s" % (kebab, ext): service_code,
        }

        result = {
            "files": files,
            "notes": [
                "Standard NestJS trio: %s (@Module), %s (@Controller '%s'), %s (@Injectable)." % (module, ctrl, route, svc),
                "Controller has @Get/@Get(':id')/@Post/@Put(':id')/@Delete(':id') with ParseIntPipe param parsing.",
                "Service holds an in-memory array with CRUD stubs; controller + service wired into the module.",
            ],
            "next_steps": [
                "Import %s into your AppModule's `imports: []` array." % module,
                "Replace the `any` DTOs with a create/update DTO class + class-validator decorators.",
            ],
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "nestjs_scaffold failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
