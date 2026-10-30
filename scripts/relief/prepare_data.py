#!/usr/bin/env python3
"""Prepare the local relief from ČÚZK SHP ZIPs and an OSM map XML extract.

Install pyshp, pyproj and shapely in a separate Python environment. See
 docs/relief.md for exact inputs, provenance and licensing.
"""
import argparse
import json
import xml.etree.ElementTree as ET
from collections import Counter
from pathlib import Path
from tempfile import TemporaryDirectory
from zipfile import ZipFile
import shapefile
from pyproj import Transformer
from shapely.geometry import LineString, Point, Polygon, box, shape as geometry_shape
from shapely.ops import unary_union, split
from shapely.prepared import prep

parser = argparse.ArgumentParser()
parser.add_argument('--ruian', required=True)
parser.add_argument('--cadastre', required=True)
parser.add_argument('--osm', required=True)
parser.add_argument('--output', default='public/relief/data.js')
args = parser.parse_args()
transform = Transformer.from_crs(5514, 4326, always_xy=True)
def coord(point):
    lon, lat = transform.transform(*point[:2])
    return [round(lon, 7), round(lat, 7)]
with TemporaryDirectory() as directory:
    for archive in [args.ruian, args.cadastre]:
        with ZipFile(archive) as source:
            source.extractall(directory)
    root = Path(directory)
    def reader(name):
        return shapefile.Reader(str(name), encoding='1250')
    boundary = reader(root / '577685/OBEC_P').shape(0)
    parts = list(boundary.parts) + [len(boundary.points)]
    bounds = [[coord(p) for p in boundary.points[a:b]] for a, b in zip(parts, parts[1:])]
    places = []
    for item in reader(root / '577685/CO_B').iterShapeRecords():
        places.append({'name': item.record['NAZEV'], 'position': coord(item.shape.points[0])})
    buildings = []
    for item in reader(root / '787744/BUDOVY_P').iterShapeRecords():
        shape = item.shape
        parts = list(shape.parts) + [len(shape.points)]
        rings = [[coord(p) for p in shape.points[a:b]] for a,b in zip(parts,parts[1:])]
        buildings.append({'rings':rings, 'source_id':str(item.record['SO_KOD']), '_geom':geometry_shape(shape.__geo_interface__)})

# Preserve OSM IDs and road classification in the distributed extract.
nodes = {}
roads = []
waterways = []
water_areas = []
woodlands = []
localities = []
osm_buildings = {}
hiking_relations = []
# Selected named features, retained with OSM IDs so their provenance is auditable.
node_names = {
    '1863175981': ('Chlum', 'Místní název'),
    '1863175988': ('Libnov', 'Místní název'),
    '1863176315': ('Škodějov', 'Místní název'),
    '4715546181': ('Na Kněžsku', 'Místní název'),
    '7761759793': ('Stádla', 'Samota'),
    '9482920485': ('Děčín', 'Samota'),
    '9482920486': ('Za Hůrou', 'Místní název'),
    '9482920487': ('Čtvrt / Čtvrť', 'Místní název'),
    '9482920488': ('Kouty', 'Samota'),
    '9482920489': ('Podbaba', 'Samota'),
    '9482920491': ('Bukovina', 'Místní název'),
    '9482920492': ('Na Samotě', 'Samota'),
    '952966601': ('Skalanský vodopád', 'Přírodní zajímavost'),
    '3869605465': ('Hůra', 'Vrchol Vyskeř'),
}
way_names = {
    '111570482': ('Kostel Nanebevzetí Panny Marie', 'Památka'),
    '369866838': ('Dřevěná zvonice', 'Památka'),
    '359163476': ('Pekařova brána', 'Přírodní zajímavost'),
    '175763914': ('Žehrovka', 'Vodní tok'),
    '401956074': ('Čertoryje', 'Vodní tok'),
    '319241406': ('Kacanovský potok', 'Vodní tok'),
}
allowed = {'motorway', 'trunk', 'primary', 'secondary', 'tertiary', 'unclassified', 'residential', 'living_street', 'service', 'track', 'path', 'footway', 'cycleway', 'pedestrian', 'steps', 'bridleway'}
for event, elem in ET.iterparse(args.osm, events=('end',)):
    if elem.tag == 'node':
        nodes[elem.attrib['id']] = (float(elem.attrib['lon']), float(elem.attrib['lat']))
        if elem.attrib['id'] in node_names:
            name, kind = node_names[elem.attrib['id']]
            localities.append({'id': 'node/' + elem.attrib['id'], 'name': name, 'kind': kind, 'position': list(nodes[elem.attrib['id']])})
        elem.clear()
    elif elem.tag == 'way':
        tags = {x.attrib['k']: x.attrib['v'] for x in elem.findall('tag')}
        kind = tags.get('highway')
        if tags.get('ref:ruian:building'):
            osm_buildings[tags['ref:ruian:building']] = {'levels':tags.get('building:levels'), 'roof':tags.get('roof:shape'), 'building_type':tags.get('building'), 'material':tags.get('building:material'), 'id':elem.attrib['id']}
        all_points = [nodes.get(x.attrib['ref']) for x in elem.findall('nd')]
        if elem.attrib['id'] in way_names and all_points and all(all_points):
            name, label = way_names[elem.attrib['id']]
            location = LineString(all_points).centroid
            localities.append({'id': 'way/' + elem.attrib['id'], 'name': name, 'kind': label, 'position': [round(location.x, 7), round(location.y, 7)]})
        if len(all_points) >= 4 and all(all_points) and all_points[0] == all_points[-1]:
            polygon = Polygon(all_points)
            if polygon.is_valid and not polygon.is_empty:
                if tags.get('landuse') == 'forest' or tags.get('natural') == 'wood':
                    woodlands.append(polygon)
                elif tags.get('natural') == 'water' or tags.get('landuse') == 'reservoir':
                    clipped_area = polygon.intersection(box(15.12, 50.51, 15.19, 50.55))
                    areas = [clipped_area] if clipped_area.geom_type == 'Polygon' else getattr(clipped_area, 'geoms', [])
                    if elem.attrib['id'] == '50944666' and not clipped_area.is_empty:
                        location = clipped_area.representative_point()
                        localities.append({'id': 'way/50944666', 'name': 'Věžický rybník', 'kind': 'Vodní plocha', 'position': [round(location.x, 7), round(location.y, 7)]})
                    for area in areas:
                        if area.geom_type != 'Polygon' or area.is_empty: continue
                        water_areas.append({'id': int(elem.attrib['id']), 'rings': [[[round(x,7),round(y,7)] for x,y in ring.coords] for ring in [area.exterior, *area.interiors]]})
        if kind in allowed or (tags.get('waterway') in {'stream', 'river'} and not tags.get('tunnel')):

            points = [nodes.get(x.attrib['ref']) for x in elem.findall('nd')]
            # Do not create false links where a source node is missing.
            groups = []
            group = []
            for point in points + [None]:
                if point is None:
                    if len(group) >= 2: groups.append(group)
                    group = []
                else: group.append(point)
            for group in groups:
                clipped = LineString(group).intersection(box(15.12, 50.51, 15.19, 50.55))
                lines = [clipped] if clipped.geom_type == 'LineString' else getattr(clipped, 'geoms', [])
                for line in lines:
                    if line.geom_type != 'LineString' or line.is_empty: continue
                    points = line.simplify(.000012, preserve_topology=False).coords
                    record = {'id': int(elem.attrib['id']), 'type': kind or tags['waterway'], 'points': [[round(x,7),round(y,7)] for x,y in points]}
                    if kind and tags.get('ref'): record['ref'] = tags['ref']
                    if kind in allowed: roads.append(record)
                    elif tags.get('waterway') in {'stream', 'river'}: waterways.append(record)
        elem.clear()
    elif elem.tag == 'relation':
        tags = {x.attrib['k']: x.attrib['v'] for x in elem.findall('tag')}
        if tags.get('type') == 'route' and tags.get('route') in {'hiking', 'foot'}:
            for color in ('red', 'blue', 'green', 'yellow'):
                # Include KČT marked trails and their marked diversions, but not
                # educational routes with a different symbol or other networks.
                if tags.get('kct_' + color) in {'major', 'peak', 'ruin', 'spring'}:
                    hiking_relations.append({
                        'id': int(elem.attrib['id']), 'color': color,
                        'name': tags.get('name'), 'ref': tags.get('ref'),
                        'symbol': tags.get('kct_' + color),
                        'members': [int(m.attrib['ref']) for m in elem.findall('member') if m.attrib.get('type') == 'way'],
                    })
        elem.clear()

# Keep all coincident colours on a way, even if several named routes share it.
route_membership = {}
for route in hiking_relations:
    for member in route['members']:
        route_membership.setdefault(member, {}).setdefault(route['color'], set()).add(route['id'])
hiking_segments = []
for road in roads:
    colors = route_membership.get(road['id'])
    if colors:
        hiking_segments.append({'id': road['id'], 'points': road['points'],
                                'colors': sorted(colors),
                                'relations': sorted({i for ids in colors.values() for i in ids})})
route_ids = {rid for segment in hiking_segments for rid in segment['relations']}
hiking_relations = [{k:v for k,v in route.items() if k != 'members'} for route in hiking_relations if route['id'] in route_ids]

# Make a lightweight, stylized pitched roof from the footprint. Splitting the
# polygon at its long-axis ridge keeps complex footprints rather than replacing
# them with generic boxes. OSM levels are used only where an identifier matches.
for building in buildings:
    polygon = building.pop('_geom')
    osm = osm_buildings.get(building.pop('source_id'), {})
    try:
        levels = float(osm.get('levels') or 0)
        building['levels'] = levels if .5 <= levels <= 5 else None
    except ValueError:
        building['levels'] = None
    building['osm_id'] = int(osm['id']) if osm.get('id') else None
    building['roof_type'] = osm.get('roof')
    building['building_type'] = osm.get('building_type')
    building['material'] = osm.get('material')
    building['roof'] = []
    if polygon.geom_type != 'Polygon' or not polygon.is_valid or polygon.area < 8 or osm.get('roof') == 'flat':
        continue
    rect = list(polygon.minimum_rotated_rectangle.exterior.coords)[:4]
    edges = [(rect[(i+1)%4][0]-rect[i][0], rect[(i+1)%4][1]-rect[i][1]) for i in range(4)]
    along = max(edges, key=lambda e:e[0]**2+e[1]**2)
    length = (along[0]**2+along[1]**2)**.5
    if length < 1: continue
    ux,uy = along[0]/length,along[1]/length
    nx,ny = -uy,ux
    center = polygon.minimum_rotated_rectangle.centroid
    half_width = max(abs((x-center.x)*nx+(y-center.y)*ny) for x,y in rect)
    if half_width < 1: continue
    building['roof_span_m'] = round(half_width*2, 1)
    ridge = LineString([(center.x-ux*length*2,center.y-uy*length*2),(center.x+ux*length*2,center.y+uy*length*2)])
    pieces = split(polygon, ridge).geoms
    for piece in pieces:
        if piece.geom_type != 'Polygon' or piece.is_empty: continue
        roof_rings=[]
        for ring in [piece.exterior,*piece.interiors]:
            roof_rings.append([coord((x,y))+[round(max(0,min(1,1-abs((x-center.x)*nx+(y-center.y)*ny)/half_width)),3)] for x,y in ring.coords])
        building['roof'].append(roof_rings)

# Sample mapped woodland against the terrain grid. The treeless Hůra treatment
# follows the project's established visual direction, so its summit is omitted.
forest = prep(unary_union(woodlands))
forest_mask = []
for y in range(100):
    lat = 50.55 - .04 * y / 99
    row = ''
    for x in range(140):
        lon = 15.12 + .07 * x / 139
        distance = ((lon - 15.159747) * 70_700)**2 + ((lat - 50.531955) * 111_000)**2
        row += '1' if distance > 300**2 and forest.contains(Point(lon, lat)) else '0'
    forest_mask.append(row)
result = {'boundary':bounds,'places':places,'buildings':buildings,'roads':roads,
          'localities':localities,'waterways':waterways,'water_areas':water_areas,
          'forest_mask':forest_mask,'hiking_segments':hiking_segments,'hiking_relations':hiking_relations}
Path(args.output).write_text('export default ' + json.dumps(result, ensure_ascii=False, separators=(',',':')) + ';\n')
print('places',len(places),'boundary rings',len(bounds),'buildings',len(buildings),'known levels',sum(b['levels'] is not None for b in buildings),'pitched roofs',sum(bool(b['roof']) for b in buildings),'roads',len(roads),Counter(r['type'] for r in roads),'localities',len(localities),'waterways',len(waterways),'water areas',len(water_areas),'woods',len(woodlands),'forest samples',sum(row.count('1') for row in forest_mask),'hiking segments',len(hiking_segments),'hiking relations',len(hiking_relations))
