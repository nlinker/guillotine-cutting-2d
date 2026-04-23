from toolkit import *

seed(0)

k = 10

surface = Shape(k, k)
blocks = list(random_shape(surface) for i in range(k))

S = Slicer(surface, blocks)
print(S)

S.solve()
S.report_coverage()
