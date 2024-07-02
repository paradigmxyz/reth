import { Node } from "./node";
/**
 * Return the `Node` at a specified height from the merkle tree made of "zero data"
 * ```
 *           ...
 *          /
 *         x           <- height 2
 *      /     \
 *     x       x       <- height 1
 *   /  \      /  \
 * 0x0  0x0  0x0  0x0  <- height 0
 * ```
 */
export declare function zeroNode(height: number): Node;
