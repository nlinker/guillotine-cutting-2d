from toolkit import *

M = 4
N = 5

surface = Shape(M, N)
blocks = [Shape(3, 2), Shape(2, 2), Shape(2, 1), Shape(2, 3), Shape(1, 2)]

S = Slicer(surface, blocks)
print(S)

S.solve()
