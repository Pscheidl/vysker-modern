"""Vstupní HTML pro přímé odkazy a obnovení stránek na GitHub Pages."""
import json
import os
from pathlib import Path
import re
import shutil

root = Path(__file__).resolve().parent.parent
output = Path(os.environ["TRUNK_STAGING_DIR"])
entry = output / "index.html"
routes = re.findall(r'path!\("([^"/:]*)"\)', (root / "src/app/mod.rs").read_text())
documents = json.loads((root / "demo/documents.json").read_text())
routes.extend(f"uredni-deska/{notice['id']}" for notice in documents["notices"])

for route in routes:
    if route:
        destination = output / route / "index.html"
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(entry, destination)

# Neznámé adresy mají skutečný stav 404 a zobrazí naši stránku nenalezeno.
shutil.copyfile(entry, output / "404.html")
(output / ".nojekyll").touch()
