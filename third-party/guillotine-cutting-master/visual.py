from toolkit import *
import pygame


WIDTH = 1200
HEIGHT = 900
RUNNING = True

SIDE = 600
K = 10

# WINDOW
window = pygame.display.set_mode((WIDTH, HEIGHT))
pygame.display.set_caption('Guillotine problem algorithm visualisation')
screen = pygame.Surface((WIDTH, HEIGHT))
draw_area = pygame.Surface((SIDE, SIDE))
draw_area.fill((255, 255, 255))
pygame.font.init()


surface = Shape(K, K)
blocks = list(random_shape(surface) for i in range(K))
blocks = [Shape(4, 3),
          Shape(2, 2),
          Shape(2, 3),
          Shape(1, 5),
          Shape(3, 2),
          Shape(3, 3),
          Shape(4, 4),
          Shape(3, 4),
          Shape(4, 3),
          Shape(6, 1),
          Shape(6, 2),
          ]
S = Slicer(surface, blocks)
S.solve()
S.report_coverage()


def grid(area, k):
    unit = int(SIDE / k)
    for row in range(0, SIDE, unit):
        pygame.draw.line(area, (150, 150, 150), (row, 0), (row, SIDE))
    for line in range(0, SIDE, unit):
        pygame.draw.line(area, (150, 150, 150), (0, line), (SIDE, line))


while RUNNING:
    for e in pygame.event.get():
        if e.type == pygame.QUIT:
            RUNNING = False

    grid(draw_area, K)
    S.draw_solution(draw_area, int(SIDE / K), (0, 0), pygame.draw.line, pygame.draw.rect)

    screen.fill((150, 150, 200))
    screen.blit(draw_area, ((WIDTH-SIDE)/2, (HEIGHT-SIDE)/2))
    window.blit(screen, (0, 0))
    pygame.display.flip()
