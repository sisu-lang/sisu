import math

SOLAR_MASS = 4 * math.pi * math.pi
DAYS_PER_YEAR = 365.24


class Body:
    def __init__(self, x, y, z, vx, vy, vz, mass):
        self.x = x
        self.y = y
        self.z = z
        self.vx = vx
        self.vy = vy
        self.vz = vz
        self.mass = mass


def planet(x, y, z, vx, vy, vz, mass):
    return Body(
        x, y, z, vx * DAYS_PER_YEAR, vy * DAYS_PER_YEAR, vz * DAYS_PER_YEAR, mass * SOLAR_MASS
    )


def offset_momentum(bodies):
    px = py = pz = 0.0
    for b in bodies:
        px += b.vx * b.mass
        py += b.vy * b.mass
        pz += b.vz * b.mass
    bodies[0].vx = -px / SOLAR_MASS
    bodies[0].vy = -py / SOLAR_MASS
    bodies[0].vz = -pz / SOLAR_MASS


def energy(bodies):
    e = 0.0
    for i, a in enumerate(bodies):
        e += 0.5 * a.mass * (a.vx * a.vx + a.vy * a.vy + a.vz * a.vz)
        for b in bodies[i + 1 :]:
            dx = a.x - b.x
            dy = a.y - b.y
            dz = a.z - b.z
            e -= a.mass * b.mass / math.sqrt(dx * dx + dy * dy + dz * dz)
    return e


def advance(bodies, dt):
    for i, a in enumerate(bodies):
        for b in bodies[i + 1 :]:
            dx = a.x - b.x
            dy = a.y - b.y
            dz = a.z - b.z
            d2 = dx * dx + dy * dy + dz * dz
            mag = dt / (d2 * math.sqrt(d2))
            a.vx -= dx * b.mass * mag
            a.vy -= dy * b.mass * mag
            a.vz -= dz * b.mass * mag
            b.vx += dx * a.mass * mag
            b.vy += dy * a.mass * mag
            b.vz += dz * a.mass * mag
    for b in bodies:
        b.x += dt * b.vx
        b.y += dt * b.vy
        b.z += dt * b.vz


def main():
    steps = 5_000_000
    bodies = [
        Body(0.0, 0.0, 0.0, 0.0, 0.0, 0.0, SOLAR_MASS),
        planet(
            4.84143144246472090,
            -1.16032004402742839,
            -1.03622044471123109e-1,
            1.66007664274403694e-3,
            7.69901118419740425e-3,
            -6.90460016972063023e-5,
            9.54791938424326609e-4,
        ),
        planet(
            8.34336671824457987,
            4.12479856412430479,
            -4.03523417114321381e-1,
            -2.76742510726862411e-3,
            4.99852801234917238e-3,
            2.30417297573763929e-5,
            2.85885980666130812e-4,
        ),
        planet(
            1.28943695621391310e1,
            -1.51111514016986312e1,
            -2.23307578892655734e-1,
            2.96460137564761618e-3,
            2.37847173959480950e-3,
            -2.96589568540237556e-5,
            4.36624404335156298e-5,
        ),
        planet(
            1.53796971148509165e1,
            -2.59193146099879641e1,
            1.79258772950371181e-1,
            2.68067772490389322e-3,
            1.62824170038242295e-3,
            -9.51592254519715870e-5,
            5.15138902046611451e-5,
        ),
    ]
    offset_momentum(bodies)
    print(f"{energy(bodies):.9f}")
    for _ in range(steps):
        advance(bodies, 0.01)
    print(f"{energy(bodies):.9f}")


main()
