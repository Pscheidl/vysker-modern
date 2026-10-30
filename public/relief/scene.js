import * as THREE from './three.module.js';
import {OrbitControls} from './OrbitControls.js';
import terrain from './terrain.js';
import data from './data.js';
import {terrainSurfaceHeight as sampleSurface} from './terrain-surface.js';

export function mount(container) {
const order=['Vyskeř','Drahoňovice','Lažany','Mladostov','Poddoubí','Skalany'];
const places=order.map(name=>{const place=data.places.find(item=>item.name===name);return {name,lon:place.position[0],lat:place.position[1]}});
places.push({name:'Kaple svaté Anny',lon:15.15976,lat:50.53194});
places.push(...data.localities.slice().sort((a,b)=>a.name.localeCompare(b.name,'cs')).map(item=>({name:item.name,lon:item.position[0],lat:item.position[1],kind:item.kind,sourceId:item.id,locality:true})));
const facts={
 'Vyskeř':{text:'První písemná zmínka o Vyskři pochází z roku 1318. Obec tvoří šest částí: Vyskeř, Drahoňovice, Lažany, Mladostov, Poddoubí a Skalany.',url:'https://www.mistopisy.cz/pruvodce/obec/10310/vysker/',source:'Místopisy.cz'},
 'Lažany':{text:'V místní části Lažany najdete Pekařovu bránu, krátký silniční průjezd vytesaný v roce 1914 do skály Mnich.',url:'https://www.turnovskovakci.cz/obce-skoly/vysker-obec-s-dalekymi-vyhledy/',source:'Turnovsko v akci'},
 'Mladostov':{text:'Nedaleko Mladostova v prostoru obce Vyskeř se nachází Skalanský vodopád.',url:'https://www.regionem.cz/clanek/kacanovy-za-vyhledem-skalni-galerii-i-vodopadem-ceskeho-raje/',source:'Regionem.cz'},
 'Hůra':{text:'Vrch Vyskeř, místně Hůra, dosahuje podle turistického popisu 465 metrů. Od kostela na vrchol stoupá křížová cesta se čtrnácti zastaveními.',url:'https://www.cesky-raj.info/vysker-kaple',source:'Český ráj'},
 'Kaple svaté Anny':{text:'Kaple na Hůře pochází ze druhé poloviny 17. století. Současnou podobu získala při obnově v roce 1830.',url:'https://www.cesky-raj.info/vysker-kaple',source:'Český ráj'},
 'Kostel Nanebevzetí Panny Marie':{text:'Novorománský kostel tvoří společně se sousední dřevěnou zvonicí jednu z dominant obce.',url:'https://www.cesky-raj.info/cile/vysker',source:'Český ráj'},
 'Dřevěná zvonice':{text:'Osmiboká dřevěná zvonice u kostela je datována do roku 1504. Je v ní zavěšen zvon Václav.',url:'https://www.cesky-raj.info/cile/vysker',source:'Český ráj'},
 'Pekařova brána':{text:'Lomený průjezd byl vytesán do pískovcové skály Mnich při stavbě silnice v roce 1914. Je pojmenován po historikovi Josefu Pekařovi.',url:'https://www.cesky-raj.info/cile/vysker',source:'Český ráj'},
 'Věžický rybník':{text:'Věžický rybník neboli Věžák je součástí Podtroseckých údolí. Pískovcové skály vystupují přímo z jeho hladiny. Model zachycuje jen západní část rybníka.',url:'https://www.cesky-raj.info/vezicky-rybnik',source:'Český ráj'},
 'Skalanský vodopád':{text:'Vodopád leží v lesnatém údolí poblíž Mladostova. Jeho poloha je v modelu převzatá z OpenStreetMap.',url:'https://www.openstreetmap.org/node/952966601',source:'OpenStreetMap'}
};
const viewport=container.querySelector('#relief-viewport'),labels=container.querySelector('#relief-labels'),nav=container.querySelector('#relief-places'),localNav=container.querySelector('#relief-localities');
const root=document.documentElement;
let dark=root.dataset.theme==='dark';
let frame=0, observer, themeObserver, visibilityObserver, renderer;
let visible=true;
let requestRender=()=>{};
const query=(selector)=>container.querySelector(selector);
const sceneWidth=10, sceneDepth=9, exaggeration=2.0, minHeight=240;
const [west,south,east,north]=terrain.bounds;
function scenePoint(lon,lat){return new THREE.Vector3((lon-west)/(east-west)*sceneWidth-sceneWidth/2,0,(north-lat)/(north-south)*sceneDepth-sceneDepth/2)}
function altitude(lon,lat){const u=Math.max(0,Math.min(terrain.width-1,(lon-west)/(east-west)*(terrain.width-1))),v=Math.max(0,Math.min(terrain.height-1,(north-lat)/(north-south)*(terrain.height-1)));const x=Math.floor(u),y=Math.floor(v),x1=Math.min(x+1,terrain.width-1),y1=Math.min(y+1,terrain.height-1),a=u-x,b=v-y;const val=(x,y)=>terrain.values[y*terrain.width+x];return (val(x,y)*(1-a)*(1-b)+val(x1,y)*a*(1-b)+val(x,y1)*(1-a)*b+val(x1,y1)*a*b-minHeight)*sceneWidth/4940*exaggeration}
function point(lon,lat){const p=scenePoint(lon,lat);p.y=altitude(lon,lat);return p}
const terrainSurfaceHeight=(x,z)=>sampleSurface(terrain,x,z,sceneWidth,sceneDepth,minHeight,exaggeration);

try {
 const scene=new THREE.Scene();
 const camera=new THREE.PerspectiveCamera(35,1,0.1,100);
 camera.position.set(10,11,15);
 renderer=new THREE.WebGLRenderer({antialias:true,alpha:true, powerPreference:"low-power"});
 renderer.setPixelRatio(Math.min(window.devicePixelRatio,1.5));renderer.outputColorSpace=THREE.SRGBColorSpace;
 viewport.prepend(renderer.domElement);
 const controls=new OrbitControls(camera,renderer.domElement);controls.enableDamping=true;controls.dampingFactor=.09;controls.minDistance=.55;controls.maxDistance=27;controls.maxPolarAngle=Math.PI*.47;controls.enablePan=true;controls.target.set(0,.25,0);controls.update();
 scene.add(new THREE.HemisphereLight(0xffffff,0x9daea0,2));
 const sun=new THREE.DirectionalLight(0xffffff,2.2);sun.position.set(-8,14,9);scene.add(sun);
 const w=terrain.width,h=terrain.height;
 const vertices=[],indices=[],colors=[];
 for(let row=0;row<h;row++) for(let col=0;col<w;col++) {
   const lon=west+(east-west)*col/(w-1),lat=north-(north-south)*row/(h-1),p=point(lon,lat);vertices.push(p.x,p.y,p.z);
   const height=terrain.values[row*w+col];const t=Math.max(0,Math.min(1,(height-260)/180));const forest=data.forest_mask[row][col]==='1';const c=new THREE.Color().setRGB((.70+.19*t)*(forest?.74:1),(.76+.17*t)*(forest?.84:1),(.68+.20*t)*(forest?.74:1));colors.push(c.r,c.g,c.b);
 }
 for(let y=0;y<h-1;y++) for(let x=0;x<w-1;x++){const a=y*w+x,b=a+1,c=a+w,d=c+1;indices.push(a,c,b,b,c,d)}
 const geometry=new THREE.BufferGeometry();geometry.setAttribute('position',new THREE.Float32BufferAttribute(vertices,3));geometry.setAttribute('color',new THREE.Float32BufferAttribute(colors,3));geometry.setIndex(indices);geometry.computeVertexNormals();
 const landMaterial=new THREE.MeshStandardMaterial({vertexColors:true,roughness:1});const land=new THREE.Mesh(geometry,landMaterial);scene.add(land);
 const sideMaterial=new THREE.MeshStandardMaterial({color:0xd4dbd2,roughness:1,side:THREE.DoubleSide});
 let edge=[];for(let x=0;x<w;x++)edge.push(x);for(let y=1;y<h;y++)edge.push(y*w+w-1);for(let x=w-2;x>=0;x--)edge.push((h-1)*w+x);for(let y=h-2;y>0;y--)edge.push(y*w);
 const sidePos=[];for(let i=0;i<edge.length;i++){const a=edge[i],b=edge[(i+1)%edge.length];const v=(id)=>[vertices[id*3],vertices[id*3+1],vertices[id*3+2]];const A=v(a),B=v(b),A0=[A[0],-.13,A[2]],B0=[B[0],-.13,B[2]];sidePos.push(...A,...B,...A0,...B,...B0,...A0)}
 const sideGeometry=new THREE.BufferGeometry();sideGeometry.setAttribute('position',new THREE.Float32BufferAttribute(sidePos,3));sideGeometry.computeVertexNormals();scene.add(new THREE.Mesh(sideGeometry,sideMaterial));
 const base=new THREE.Mesh(new THREE.BoxGeometry(sceneWidth,.13,sceneDepth),sideMaterial);base.position.y=-.195;scene.add(base);
 // Footprints and available building types and storeys are mapped data.
 // Roof pitch, exterior colours, openings and their positions are stylized.
 const buildingMat=new THREE.MeshStandardMaterial({color:0xffffff,vertexColors:true,roughness:1,side:THREE.DoubleSide});
 const roofMat=new THREE.MeshStandardMaterial({color:0xffffff,vertexColors:true,roughness:1,side:THREE.DoubleSide});
 const windowMatHouse=new THREE.MeshStandardMaterial({color:0x52645b,emissive:0xffc36d,emissiveIntensity:0,side:THREE.DoubleSide});
 const windowTrimMat=new THREE.MeshStandardMaterial({color:0xe8e6db,roughness:1,side:THREE.DoubleSide});
 const doorMat=new THREE.MeshStandardMaterial({color:0x715c4b,roughness:1,side:THREE.DoubleSide});
 const wallVertices=[],wallColors=[],roofVertices=[],roofColors=[],windowVertices=[],trimVertices=[],doorVertices=[];
 const roofs=[0x718277,0x9d7c69,0x6d7774,0x95675a,0x8b9485].map(c=>new THREE.Color(c));
 const plaster=[0xf2efe4,0xe9e5d8,0xf4f1e9,0xe4e5dc].map(c=>new THREE.Color(c));
 const wood=new THREE.Color(0xb2a394),utility=new THREE.Color(0xd3d1c6);
 const residential=new Set(['residential','house','apartments','hotel','cabin']);
 const utilityTypes=new Set(['barn','farm_auxiliary','garage','industrial','shed']);
 const addColor=(array,color,count)=>{for(let i=0;i<count;i++)array.push(color.r,color.g,color.b)};
 const triangleQuad=(array,x,z,dx,dz,nx,nz,y1,y2,halfWidth)=>{
   const ax=x-dx*halfWidth+nx,az=z-dz*halfWidth+nz,bx=x+dx*halfWidth+nx,bz=z+dz*halfWidth+nz;
   array.push(ax,y1,az,bx,y1,bz,ax,y2,az,bx,y1,bz,bx,y2,bz,ax,y2,az);
 };
 const pointOnEdge=(point,outline)=>{
   for(let i=0;i<outline.length-1;i++){const a=outline[i],b=outline[i+1],dx=b.x-a.x,dz=b.z-a.z,len2=dx*dx+dz*dz;if(!len2)continue;const t=Math.max(0,Math.min(1,((point.x-a.x)*dx+(point.z-a.z)*dz)/len2));if(Math.hypot(point.x-a.x-t*dx,point.z-a.z-t*dz)<.00035)return true}
   return false;
 };
 for(const [buildingIndex,building] of data.buildings.entries()){
   const rings=building.rings,outline=rings[0];if(outline.length<4)continue;
   const center=outline.reduce((p,c)=>[p[0]+c[0]/outline.length,p[1]+c[1]/outline.length],[0,0]);
   // The chapel has a distinct hand-built model.
   if(Math.abs(center[0]-15.15976)<.00025&&Math.abs(center[1]-50.53194)<.00018)continue;
   const projected=outline.map(p=>scenePoint(...p));
   const base=Math.max(...outline.map(p=>altitude(...p)))+.002;
   const levels=building.levels||1.5,height=.004+levels*.008;
   const pitch=Math.max(.003,Math.min(.014,(building.roof_span_m||7)*sceneWidth/4940*.38));
   const roofColor=roofs[buildingIndex%roofs.length];
   const wallColor=building.material==='wood'||['barn','farm_auxiliary','cabin'].includes(building.building_type)?wood:utilityTypes.has(building.building_type)?utility:plaster[buildingIndex%plaster.length];
   const roofPieces=building.roof.length?building.roof:rings.length?[rings.map(r=>r.map(p=>[...p,0]))]:[];
   for(const roofRings of roofPieces){
     const roofShape=new THREE.Shape(),samples=[];
     const addRing=(path,ring)=>{ring.forEach(([lon,lat,factor],i)=>{const p=scenePoint(lon,lat);samples.push([p.x,p.z,factor]);if(i===0)path.moveTo(p.x,p.z);else path.lineTo(p.x,p.z)});path.closePath()};
     addRing(roofShape,roofRings[0]);for(const ring of roofRings.slice(1)){const hole=new THREE.Path();addRing(hole,ring);roofShape.holes.push(hole)}
     const top=new THREE.ShapeGeometry(roofShape),pos=top.attributes.position,index=top.index;
     const add=(n)=>{const x=pos.getX(n),z=pos.getY(n);let factor=0,nearest=Infinity;for(const p of samples){const dist=(x-p[0])**2+(z-p[1])**2;if(dist<nearest){nearest=dist;factor=p[2]}}roofVertices.push(x,base+height+pitch*factor,z);roofColors.push(roofColor.r,roofColor.g,roofColor.b)};
     if(index)for(let i=0;i<index.count;i++)add(index.getX(i));else for(let i=0;i<pos.count;i++)add(i);top.dispose();
     // Close the gables where the sloping roof meets the footprint perimeter.
     for(const ring of roofRings)for(let i=0;i<ring.length-1;i++){
       const a=scenePoint(...ring[i]),b=scenePoint(...ring[i+1]);if(!pointOnEdge(a,projected)||!pointOnEdge(b,projected))continue;
       const ay=base+height+pitch*ring[i][2],by=base+height+pitch*ring[i+1][2];
       if(ay<=base+height+.00001&&by<=base+height+.00001)continue;
       wallVertices.push(a.x,base+height,a.z,b.x,base+height,b.z,a.x,ay,a.z,b.x,base+height,b.z,b.x,by,b.z,a.x,ay,a.z);addColor(wallColors,wallColor,6);
     }
   }
   let longest=-1,longestIndex=-1;
   for(let i=0;i<outline.length-1;i++){const a=projected[i],b=projected[i+1],length=Math.hypot(b.x-a.x,b.z-a.z);if(length>longest){longest=length;longestIndex=i}}
   for(const [ringIndex,ring] of rings.entries()){
     const signed=ring.slice(0,-1).reduce((total,p,i)=>{const a=scenePoint(...p),b=scenePoint(...ring[i+1]);return total+a.x*b.z-b.x*a.z},0),side=signed>=0?1:-1;
     for(let i=0;i<ring.length-1;i++){
       const a=scenePoint(...ring[i]),b=scenePoint(...ring[i+1]),aGround=altitude(...ring[i])-.003,bGround=altitude(...ring[i+1])-.003;
       wallVertices.push(a.x,aGround,a.z,b.x,bGround,b.z,a.x,base+height,a.z,b.x,bGround,b.z,b.x,base+height,b.z,a.x,base+height,a.z);addColor(wallColors,wallColor,6);
       const dx=b.x-a.x,dz=b.z-a.z,len=Math.hypot(dx,dz);if(ringIndex||len<.016||utilityTypes.has(building.building_type))continue;
       const ux=dx/len,uz=dz/len,nx=uz*side*.0007,nz=-ux*side*.0007;
       const count=Math.min(5,Math.max(1,Math.floor(len/.02))),floors=Math.max(1,Math.min(3,Math.round(levels)));
       for(let floor=0;floor<floors;floor++)for(let n=0;n<count;n++){
         const t=(n+1)/(count+1),x=a.x+dx*t,z=a.z+dz*t,y=base+height*(floor+.56)/floors,wh=Math.min(.0035,height/(floors*4)),ww=Math.min(.0021,len/(count*7));
         triangleQuad(trimVertices,x,z,ux,uz,nx*.8,nz*.8,y-wh-.0005,y+wh+.0005,ww+.0005);
         triangleQuad(windowVertices,x,z,ux,uz,nx,nz,y-wh,y+wh,ww);
       }
       if(i===longestIndex&&len>.022&&(residential.has(building.building_type)||!building.building_type)){
         const x=a.x+dx*.5,z=a.z+dz*.5,ground=Math.max(aGround,bGround),doorHeight=Math.min(height*.42,.008);
         triangleQuad(doorVertices,x,z,ux,uz,nx,nz,ground,ground+doorHeight,.0022);
       }
     }
   }
 }
 function mesh(vertices,material,colors){const g=new THREE.BufferGeometry();g.setAttribute('position',new THREE.Float32BufferAttribute(vertices,3));if(colors)g.setAttribute('color',new THREE.Float32BufferAttribute(colors,3));g.computeVertexNormals();const m=new THREE.Mesh(g,material);scene.add(m);return m}
 mesh(wallVertices,buildingMat,wallColors);mesh(roofVertices,roofMat,roofColors);mesh(trimVertices,windowTrimMat);mesh(windowVertices,windowMatHouse);mesh(doorVertices,doorMat);
 // Drape lines closely over the terrain and account for their lateral offset.
 function ribbon(lines,width,color,lift=.002){
   const out=[];
   for(const item of lines){
     const line=Array.isArray(item)?item:item.points,offset=Array.isArray(item)?0:item.offset;
     for(let i=0;i<line.length-1;i++){
       const a=line[i],b=line[i+1],pa=scenePoint(...a),pb=scenePoint(...b),length=pa.distanceTo(pb);
       if(length<1e-9)continue;
       const count=Math.max(1,Math.ceil(length/.01)),nx=-(pb.z-pa.z)/length,nz=(pb.x-pa.x)/length;
       for(let j=0;j<count;j++){
         const t=j/count,u=(j+1)/count;
         const p=new THREE.Vector3(pa.x+(pb.x-pa.x)*t+nx*offset,0,pa.z+(pb.z-pa.z)*t+nz*offset);
         const q=new THREE.Vector3(pa.x+(pb.x-pa.x)*u+nx*offset,0,pa.z+(pb.z-pa.z)*u+nz*offset);
         const half=width/2,ax=p.x+nx*half,az=p.z+nz*half,bx=p.x-nx*half,bz=p.z-nz*half,cx=q.x+nx*half,cz=q.z+nz*half,dx=q.x-nx*half,dz=q.z-nz*half;
         const ay=terrainSurfaceHeight(ax,az)+lift,by=terrainSurfaceHeight(bx,bz)+lift,cy=terrainSurfaceHeight(cx,cz)+lift,dy=terrainSurfaceHeight(dx,dz)+lift;
         out.push(ax,ay,az,bx,by,bz,cx,cy,cz,bx,by,bz,dx,dy,dz,cx,cy,cz);
       }
     }
   }
   return mesh(out,new THREE.MeshBasicMaterial({color,side:THREE.DoubleSide}));
 }
 // OSM classes are a cartographic guide, not evidence of legal ownership.
 const regional=data.roads.filter(r=>['primary','secondary','tertiary'].includes(r.type)).map(r=>r.points);
 const local=data.roads.filter(r=>['unclassified','residential','living_street'].includes(r.type)).map(r=>r.points);
 const minor=data.roads.filter(r=>!['primary','secondary','tertiary','unclassified','residential','living_street'].includes(r.type)).map(r=>r.points);
 const roadMinor=ribbon(minor,.010,0x817b70,.0015),roadLocal=ribbon(local,.019,0x7d609c,.002),roadRegional=ribbon(regional,.027,0xb85e24,.0025),border=ribbon(data.boundary,.015,0x47835f,.0018);
 // Center single routes on the mapped path and separate shared colours slightly.
 const trailColors={red:0xc72b31,blue:0x1767bc,green:0x19834d,yellow:0xd5a51b};
 const trailOrder=['red','blue','green','yellow'],trailMeshes=[];
 for(const color of trailOrder){
   const segments=data.hiking_segments.filter(s=>s.colors.includes(color)).map(segment=>{
     const colors=trailOrder.filter(c=>segment.colors.includes(c));
     return {points:segment.points,offset:(colors.indexOf(color)-(colors.length-1)/2)*.009};
   });
   if(segments.length){trailMeshes.push(ribbon(segments,.008,0xffffff,.004));trailMeshes.push(ribbon(segments,.0045,trailColors[color],.0045))}
 }
 const trailToggle=query('#relief-trails');
 const toggleTrails=()=>{for(const m of trailMeshes)m.visible=trailToggle.checked;requestRender()};
 trailToggle.addEventListener('change',toggleTrails);toggleTrails();
 const waterLine=ribbon(data.waterways.map(r=>r.points),.013,0x77a9b0,.0018);
 // Ponds are simplified shapes draped on the sampled terrain.
 const waterVertices=[];
 for(const area of data.water_areas){
   const rings=area.rings;if(!rings.length||rings[0].length<4)continue;
   const waterShape=new THREE.Shape();rings[0].forEach(([lon,lat],i)=>{const p=scenePoint(lon,lat);if(i===0)waterShape.moveTo(p.x,p.z);else waterShape.lineTo(p.x,p.z)});waterShape.closePath();
   for(const ring of rings.slice(1)){const hole=new THREE.Path();ring.forEach(([lon,lat],i)=>{const p=scenePoint(lon,lat);if(i===0)hole.moveTo(p.x,p.z);else hole.lineTo(p.x,p.z)});hole.closePath();waterShape.holes.push(hole)}
   const surface=new THREE.ShapeGeometry(waterShape);const pos=surface.attributes.position,index=surface.index;
   const add=(i)=>{const x=pos.getX(i),z=pos.getY(i);const lon=west+(x+sceneWidth/2)/sceneWidth*(east-west),lat=north-(z+sceneDepth/2)/sceneDepth*(north-south);waterVertices.push(x,altitude(lon,lat)+.009,z)};
   if(index)for(let i=0;i<index.count;i++)add(index.getX(i));else for(let i=0;i<pos.count;i++)add(i);
   surface.dispose();
 }
 const waterSurface=mesh(waterVertices,new THREE.MeshBasicMaterial({color:0x9bc2c2,side:THREE.DoubleSide}));
 for(const item of places){
   const center=point(item.lon,item.lat);
   item.position=center.clone();item.position.y+=item.name==='Pekařova brána'?.075:.15;
   const pin=document.createElement('button');pin.className=item.locality?'pin locality-pin':'pin';pin.textContent=item.name;pin.setAttribute('aria-label','Zobrazit '+item.name);pin.tabIndex=-1;labels.append(pin);item.pin=pin;
   const button=document.createElement('button');button.className='place';button.textContent=item.name;button.setAttribute('aria-pressed','false');if(item.locality){const kind=document.createElement('span');kind.className='place-kind';kind.textContent=item.kind;button.append(kind);localNav.append(button)}else nav.append(button);item.button=button;
   const select=()=>{places.forEach(p=>{p.pin.classList.remove('active');p.button.classList.remove('active');p.button.setAttribute('aria-pressed','false')});pin.classList.add('active');button.classList.add('active');button.setAttribute('aria-pressed','true');const fact=facts[item.name]||(item.locality?{text:'Poloha místního názvu nebo zajímavosti podle údajů OpenStreetMap.',url:'https://www.openstreetmap.org/'+item.sourceId,source:'OpenStreetMap'}:{text:'Poloha části obce podle definičního bodu registru RÚIAN.',url:'https://services.cuzk.gov.cz/shp/obec/',source:'ČÚZK, RÚIAN'});query('#relief-info-name').textContent=item.name;query('#relief-info-text').textContent=fact.text;const source=query('#relief-info-source');source.href=fact.url;source.textContent=fact.source;query('#relief-place-info').hidden=false;const target=center.clone();target.y+=.12;const offset=camera.position.clone().sub(controls.target).normalize().multiplyScalar(item.name==='Pekařova brána'?.55:item.name==='Kaple svaté Anny'?1.8:item.locality?2.8:3.5);controls.target.copy(target);camera.position.copy(target).add(offset);controls.update()};pin.onclick=select;button.onclick=()=>{select();if(window.matchMedia('(max-width:760px)').matches){viewport.scrollIntoView({block:'center',behavior:window.matchMedia('(prefers-reduced-motion: reduce)').matches?'auto':'smooth'})}};
 }
 // A stylized elongated octagon, following the documented chapel silhouette.
 const chapel=point(15.15976,50.53194);
 const chapelMat=new THREE.MeshStandardMaterial({color:0xfffdf5});
 const towerMat=new THREE.MeshStandardMaterial({color:0x596b5f});
 const windowMat=new THREE.MeshStandardMaterial({color:0x52625a,emissive:0xf9b84f,emissiveIntensity:0});
 const shape=new THREE.Shape();
 const outline=[[-.12,-.065],[-.085,-.1],[.085,-.1],[.12,-.065],[.12,.065],[.085,.1],[-.085,.1],[-.12,.065]];
 shape.moveTo(...outline[0]);outline.slice(1).forEach(x=>shape.lineTo(...x));shape.closePath();
 const body=new THREE.Mesh(new THREE.ExtrudeGeometry(shape,{depth:.14,bevelEnabled:false}),chapelMat);
 body.rotation.x=-Math.PI/2;body.position.set(chapel.x,chapel.y,chapel.z);scene.add(body);
 const chapelRoofVertices=[];
 for(let i=0;i<outline.length;i++) {const a=outline[i],b=outline[(i+1)%outline.length];chapelRoofVertices.push(a[0],.14,a[1],b[0],.14,b[1],0,.23,0)}
 const roofGeometry=new THREE.BufferGeometry();roofGeometry.setAttribute('position',new THREE.Float32BufferAttribute(chapelRoofVertices,3));roofGeometry.computeVertexNormals();
 const roof=new THREE.Mesh(roofGeometry,new THREE.MeshStandardMaterial({color:0x596b5f,side:THREE.DoubleSide}));roof.position.copy(chapel);scene.add(roof);
 const lantern=new THREE.Mesh(new THREE.CylinderGeometry(.026,.026,.055,8),chapelMat);lantern.position.set(chapel.x,chapel.y+.25,chapel.z);scene.add(lantern);
 const onion=new THREE.Mesh(new THREE.SphereGeometry(.032,8,6),towerMat);onion.scale.y=1.15;onion.position.set(chapel.x,chapel.y+.292,chapel.z);scene.add(onion);
 const spire=new THREE.Mesh(new THREE.CylinderGeometry(.003,.009,.07,6),towerMat);spire.position.set(chapel.x,chapel.y+.34,chapel.z);scene.add(spire);
 const cross=new THREE.Mesh(new THREE.BoxGeometry(.05,.007,.007),towerMat);cross.position.set(chapel.x,chapel.y+.365,chapel.z);scene.add(cross);
 for (const side of [-1,1]) for (const x of [-.055,.055]) {const window=new THREE.Mesh(new THREE.PlaneGeometry(.026,.052),windowMat);window.position.set(chapel.x+x,chapel.y+.08,chapel.z+side*.101);if(side<0)window.rotation.y=Math.PI;scene.add(window)}
 // Pekařova brána: a pointed opening through a stylized sandstone outcrop.
 // The entrance is aligned with the mapped road tunnel. It is enlarged to remain visible.
 const gateLocation=[(15.1688515+15.1688083)/2,(50.5144285+50.5143647)/2];
 const gate=point(...gateLocation),gateShape=new THREE.Shape();
 gateShape.moveTo(-.16,0);gateShape.lineTo(-.17,.07);gateShape.lineTo(-.145,.16);gateShape.lineTo(-.105,.205);gateShape.lineTo(-.04,.225);gateShape.lineTo(.045,.215);gateShape.lineTo(.11,.19);gateShape.lineTo(.155,.145);gateShape.lineTo(.17,.08);gateShape.lineTo(.16,0);gateShape.lineTo(.047,0);gateShape.lineTo(.047,.065);gateShape.lineTo(.035,.085);gateShape.lineTo(0,.115);gateShape.lineTo(-.035,.085);gateShape.lineTo(-.047,.065);gateShape.lineTo(-.047,0);gateShape.closePath();
 const rockMaterial=new THREE.MeshStandardMaterial({color:0xc3b7a0,roughness:1});
 const gateModel=new THREE.Mesh(new THREE.ExtrudeGeometry(gateShape,{depth:.075,bevelEnabled:true,bevelThickness:.006,bevelSize:.004,bevelSegments:1}),rockMaterial);
 const start=scenePoint(15.1688515,50.5144285),end=scenePoint(15.1688083,50.5143647),dx=end.x-start.x,dz=end.z-start.z;
 const gateGroup=new THREE.Group();gateModel.position.z=-.0375;gateGroup.add(gateModel);
 const strata=new THREE.BufferGeometry();strata.setAttribute('position',new THREE.Float32BufferAttribute([-.145,.115,.038,-.07,.12,.038,.067,.12,.038,.145,.115,.038,-.12,.175,.038,.105,.175,.038],3));
 gateGroup.add(new THREE.LineSegments(strata,new THREE.LineBasicMaterial({color:0x9f927e})));
 gateGroup.scale.setScalar(.35);gateGroup.position.copy(gate);gateGroup.position.y+=.004;gateGroup.rotation.y=Math.atan2(dx,dz);scene.add(gateGroup);
 // A separate wooden bell tower marks the mapped building near the church.
 const bellSite=data.localities.find(p=>p.id==='way/369866838');
 if(bellSite){const bell=point(...bellSite.position);const wood=new THREE.MeshStandardMaterial({color:0x7a6758,roughness:1});const bellBase=new THREE.Mesh(new THREE.CylinderGeometry(.028,.036,.09,8),wood);bellBase.position.set(bell.x,bell.y+.045,bell.z);scene.add(bellBase);const bellRoof=new THREE.Mesh(new THREE.ConeGeometry(.045,.075,8),towerMat);bellRoof.position.set(bell.x,bell.y+.126,bell.z);scene.add(bellRoof)}
 // The church tower is a schematic landmark at the OSM building position.
 const churchSite=data.localities.find(p=>p.id==='way/111570482');
 if(churchSite){const church=point(...churchSite.position);const tower=new THREE.Mesh(new THREE.BoxGeometry(.055,.15,.055),chapelMat);tower.position.set(church.x,church.y+.075,church.z);scene.add(tower);const towerRoof=new THREE.Mesh(new THREE.ConeGeometry(.047,.09,4),towerMat);towerRoof.rotation.y=Math.PI/4;towerRoof.position.set(church.x,church.y+.195,church.z);scene.add(towerRoof);const churchCross=new THREE.Mesh(new THREE.BoxGeometry(.04,.006,.006),towerMat);churchCross.position.set(church.x,church.y+.247,church.z);scene.add(churchCross);const churchSpire=new THREE.Mesh(new THREE.BoxGeometry(.006,.04,.006),towerMat);churchSpire.position.set(church.x,church.y+.245,church.z);scene.add(churchSpire)}
 const applyTheme=()=>{dark=root.dataset.theme==='dark';sideMaterial.color.set(dark?0x47554b:0xd4dbd2);buildingMat.color.set(dark?0xaebdb0:0xffffff);windowTrimMat.color.set(dark?0xb6bcb0:0xe8e6db);doorMat.color.set(dark?0x5e6255:0x715c4b);roadRegional.material.color.set(dark?0xffbc73:0xb85e24);roadLocal.material.color.set(dark?0xd7b4ef:0x7d609c);roadMinor.material.color.set(dark?0xaaa99e:0x817b70);border.material.color.set(dark?0x9acaa6:0x47835f);waterLine.material.color.set(dark?0x79a9b4:0x648f9c);waterSurface.material.color.set(dark?0x527c82:0x9bc2c2);roofMat.color.set(dark?0xb0c2b3:0xffffff);windowMatHouse.color.set(dark?0xffd68e:0x52645b);windowMatHouse.emissiveIntensity=dark?1.1:0;landMaterial.color.set(dark?0x789282:0xffffff);windowMat.color.set(dark?0xffcd7c:0x52625a);windowMat.emissiveIntensity=dark?1.5:0;renderer.setClearColor(dark?0x1c2520:0xf3f6f2,1);requestRender()};
 applyTheme();themeObserver=new MutationObserver(applyTheme);themeObserver.observe(root,{attributes:true,attributeFilter:['data-theme']});
 query('#relief-reset').onclick=()=>{controls.target.set(0,.25,0);camera.position.set(10,11,15);places.forEach(p=>{p.pin.classList.remove('active');p.button.classList.remove('active');p.button.setAttribute('aria-pressed','false')});query('#relief-place-info').hidden=true;controls.update()};
 for (const [id,factor] of [['relief-in',.8],['relief-out',1.25]]) query('#'+id).onclick=()=>{const offset=camera.position.clone().sub(controls.target);offset.setLength(Math.max(controls.minDistance,Math.min(controls.maxDistance,offset.length()*factor)));camera.position.copy(controls.target).add(offset);controls.update()};
 for (const [id,angle] of [['relief-left',.25],['relief-right',-.25]]) query('#'+id).onclick=()=>{const offset=camera.position.clone().sub(controls.target).applyAxisAngle(new THREE.Vector3(0,1,0),angle);camera.position.copy(controls.target).add(offset);controls.update()};
 const pan=(axis,sign)=>{camera.updateMatrix();const step=camera.position.distanceTo(controls.target)*.1;const shift=new THREE.Vector3().setFromMatrixColumn(camera.matrix,axis).multiplyScalar(step*sign);camera.position.add(shift);controls.target.add(shift);controls.update()};
 for (const [id,axis,sign] of [['relief-pan-left',0,-1],['relief-pan-right',0,1],['relief-pan-up',1,1],['relief-pan-down',1,-1]]) query('#'+id).onclick=()=>pan(axis,sign);
 const keyDirections={ArrowLeft:[0,-1],ArrowRight:[0,1],ArrowUp:[1,1],ArrowDown:[1,-1]};
 const onKeyDown=(event)=>{const direction=keyDirections[event.key];if(!direction||event.altKey||event.ctrlKey||event.metaKey||event.shiftKey)return;event.preventDefault();pan(...direction)};
 const focusViewport=(event)=>{if(event.target===renderer.domElement)viewport.focus({preventScroll:true})};
 container.addEventListener('keydown',onKeyDown);viewport.addEventListener('pointerdown',focusViewport);
 const resize=()=>{const rect=viewport.getBoundingClientRect();renderer.setSize(rect.width,rect.height);camera.aspect=rect.width/rect.height;camera.updateProjectionMatrix();requestRender()};observer=new ResizeObserver(resize);observer.observe(viewport);resize();
 query('#relief-fallback').hidden=true;query('#relief-controls').hidden=false;query('#relief-compass').hidden=false;
 const render=()=>{frame=0;if(!visible||document.hidden)return;controls.update();renderer.render(scene,camera);
   const anchor=controls.target.clone(),projected=anchor.clone().project(camera),north=anchor.clone().add(new THREE.Vector3(0,0,-1)).project(camera);query('#relief-compass-arrow').style.transform=`rotate(${Math.atan2(north.x-projected.x,north.y-projected.y)}rad)`;
   // Keep labels legible as the camera moves, preferring the selected item.
   const occupied=[];const width=viewport.clientWidth,height=viewport.clientHeight;
   const sorted=places.slice().sort((a,b)=>(b.pin.classList.contains('active')?1:0)-(a.pin.classList.contains('active')?1:0));
   for(const p of sorted){const v=p.position.clone().project(camera);const x=(v.x*.5+.5)*width,y=(-v.y*.5+.5)*height;const active=p.pin.classList.contains('active');
     p.pin.style.left=x+'px';p.pin.style.top=y+'px';
     if(v.z>1||Math.abs(v.x)>1||Math.abs(v.y)>1||(p.locality&&!active&&camera.position.distanceTo(controls.target)>12)){p.pin.style.display='none';continue}
     p.pin.style.display='';const w=p.pin.offsetWidth,h=p.pin.offsetHeight;const box={left:x-w/2-4,right:x+w/2+4,top:y-h/2-3,bottom:y+h/2+3};
     if(box.left<0||box.right>width||box.top<0||box.bottom>height||occupied.some(o=>box.left<o.right&&box.right>o.left&&box.top<o.bottom&&box.bottom>o.top)){p.pin.style.display='none';continue}occupied.push(box);
   }
 };
 requestRender=()=>{if(visible&&!document.hidden&&!frame)frame=requestAnimationFrame(render)};
 controls.addEventListener('change',requestRender);
 visibilityObserver=new IntersectionObserver(entries=>{visible=entries[0].isIntersecting;requestRender()});visibilityObserver.observe(container);
 document.addEventListener('visibilitychange',requestRender);requestRender();
 renderer.domElement.addEventListener('webglcontextlost',()=>{query('#relief-fallback').hidden=false;query('#relief-controls').hidden=true;query('#relief-compass').hidden=true});
 return ()=>{cancelAnimationFrame(frame);observer.disconnect();themeObserver.disconnect();visibilityObserver.disconnect();document.removeEventListener('visibilitychange',requestRender);controls.removeEventListener('change',requestRender);trailToggle.removeEventListener('change',toggleTrails);container.removeEventListener('keydown',onKeyDown);viewport.removeEventListener('pointerdown',focusViewport);controls.dispose();scene.traverse(object=>{if(object.geometry)object.geometry.dispose();if(object.material){for(const material of [object.material].flat())material.dispose()}});renderer.dispose();renderer.domElement.remove();labels.replaceChildren();nav.replaceChildren();localNav.replaceChildren()};
} catch(e) {if(observer)observer.disconnect();if(themeObserver)themeObserver.disconnect();if(visibilityObserver)visibilityObserver.disconnect();if(renderer){renderer.dispose();renderer.domElement.remove()}labels.replaceChildren();nav.replaceChildren();localNav.replaceChildren();query('#relief-fallback').hidden=false;query('#relief-controls').hidden=true;query('#relief-compass').hidden=true;console.error('3D relief:',e)}
}
