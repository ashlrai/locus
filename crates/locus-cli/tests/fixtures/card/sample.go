package fixture
func helper(x int) int { return x + 1 }
func target(x int) int { return helper(x) }
func Test_target() int { return target(1) }
