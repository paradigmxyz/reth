import math
import matplotlib.pyplot as plt

SCALE = int(1e18)  ## fix point scale
MAX_EXP_INPUT = 135_305_999_368_893_231_588

# Python function that matches the `exp(int256 x)` function in LibFixedPointMath.sol
def fixed_point_exp(x):
    if x <= -42_139_678_854_452_767_551:
        return 0

    if x >= 135_305_999_368_893_231_589:
        raise OverflowError("Overflow")

    x = (x << 78) // (5**18)

    k = ((x << 96) // 54_916_777_467_707_473_351_141_471_128 + (2**95)) >> 96
    x = x - k * 54_916_777_467_707_473_351_141_471_128

    y = x + 1_346_386_616_545_796_478_920_950_773_328
    y = ((y * x) >> 96) + 57_155_421_227_552_351_082_224_309_758_442
    p = y + x - 94_201_549_194_550_492_254_356_042_504_812
    p = ((p * y) >> 96) + 28_719_021_644_029_726_153_956_944_680_412_240
    p = p * x + (4_385_272_521_454_847_904_659_076_985_693_276 << 96)

    q = x - 2_855_989_394_907_223_263_936_484_059_900
    q = ((q * x) >> 96) + 50_020_603_652_535_783_019_961_831_881_945
    q = ((q * x) >> 96) - 533_845_033_583_426_703_283_633_433_725_380
    q = ((q * x) >> 96) + 3_604_857_256_930_695_427_073_651_918_091_429
    q = ((q * x) >> 96) - 14_423_608_567_350_463_180_887_372_962_807_573
    q = ((q * x) >> 96) + 26_449_188_498_355_588_339_934_803_723_976_023

    r = p // q  # Integer division

    r = (r * 3_822_833_074_963_236_453_042_738_258_902_158_003_155_416_615_667) >> (
        195 - k
    )

    return r


# Test exp(1)
print("exp(1) =", fixed_point_exp(SCALE) / SCALE)
print("exp(MAX) =", fixed_point_exp(MAX_EXP_INPUT) / SCALE)

## Calculate initial gas_excess_issued
GWEI = 1e9
ETHEREUM_TARGET = 15 * 1e6
ETHEREUM_BASE_FEE = 10 * GWEI
TAIKO_TARGET = ETHEREUM_TARGET * 10
TAIKO_BASE_FEE = ETHEREUM_BASE_FEE // 10
ADJUSTMENT_QUOTIENT = 8
ADJUSTMENT_FACTOR = TAIKO_TARGET * 8


def calc_eth_qty(qty):
    return math.exp(qty / TAIKO_TARGET / ADJUSTMENT_QUOTIENT)


def calc_basefee(excess, gas_in_block):
    diff = calc_eth_qty(excess + gas_in_block) - calc_eth_qty(excess)
    return diff / gas_in_block


def calculate_excess_gas_issued(expected_base_fee, gas_used):
    numerator = expected_base_fee * gas_used / (calc_eth_qty(gas_used) - 1) + 1
    excess_gas_issued = math.log(numerator) * ADJUSTMENT_FACTOR
    return excess_gas_issued


expected_basefee = TAIKO_BASE_FEE
gas_in_block = 1
gas_excess_issued = int(calculate_excess_gas_issued(expected_basefee, gas_in_block))
print("gas_excess_issued          : ", gas_excess_issued)
print("actual_basefee             : ", calc_basefee(gas_excess_issued, gas_in_block))
print("expected_basefee           : ", expected_basefee)


# See https://ethresear.ch/t/make-eip-1559-more-like-an-amm-curve/9082
def eth_qty(gas_qty):
    v = int(int(gas_qty) * SCALE // ADJUSTMENT_FACTOR)
    if v > MAX_EXP_INPUT:
        v = MAX_EXP_INPUT
    return fixed_point_exp(v)


def calc_purchase_basefee(gas_used):
    # Returns the average base fee per gas for purchasing gas_used gas
    diff = eth_qty(gas_excess_issued + gas_used) - eth_qty(gas_excess_issued)
    return int(int(diff // gas_used) // SCALE)


def calc_spot_basefee():
    # Returns the spot price
    return int(int(eth_qty(gas_excess_issued) // SCALE) // ADJUSTMENT_FACTOR)


print("purchase basefee (1 gas) [fix point]1 : ", calc_purchase_basefee(gas_in_block))
print("spot basefee             [fix point]1 : ", calc_spot_basefee())


# Set the excess value to the max possible
bkup = gas_excess_issued
gas_excess_issued = MAX_EXP_INPUT * ADJUSTMENT_FACTOR // SCALE
print("spot basefee             [fix point]2 : ", calc_spot_basefee())


exit()
gas_excess_issued = bkup
# one L2 block per L1 block vs multiple L2 blocks per L1 block
x1 = []
y1 = []
for i in range(10):
    x1.append(i * 12)
    y1.append(calc_spot_basefee())

x2 = []
y2 = []

for i in range(10):
    for j in range(12):
        x2.append(i * 12 + j)
        y2.append(calc_spot_basefee())
        gas_excess_issued += TAIKO_TARGET / 12
    gas_excess_issued -= TAIKO_TARGET

plt.scatter(x2, y2, label="1s", color="red", marker="o")
plt.scatter(x1, y1, label="12s", color="blue", marker="x")

plt.xlabel("basefee")
plt.ylabel("time")
plt.ylim(expected_basefee * 0.75, expected_basefee * 1.25)
plt.legend()
plt.title("EIP1559 Bond Curve")
plt.show()
