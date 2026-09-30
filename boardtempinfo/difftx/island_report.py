import pcbnew, sys
b = pcbnew.LoadBoard(sys.argv[1])
pcbnew.ZONE_FILLER(b).Fill(b.Zones())
for z in b.Zones():
    if z.GetIsRuleArea(): continue
    for layer in z.GetLayerSet().Seq():
        polys = z.GetFilledPolysList(layer)
        n = polys.OutlineCount()
        if n > 1:
            print(f"zone {z.GetZoneName()} layer {b.GetLayerName(layer)}: {n} islands")
            for i in range(n):
                bb = polys.Outline(i).BBox()
                print(f"   island {i}: x {bb.GetLeft()/1e6:.1f}..{bb.GetRight()/1e6:.1f} y {bb.GetTop()/1e6:.1f}..{bb.GetBottom()/1e6:.1f}  area {polys.Outline(i).Area()/1e12:.1f} mm2")
