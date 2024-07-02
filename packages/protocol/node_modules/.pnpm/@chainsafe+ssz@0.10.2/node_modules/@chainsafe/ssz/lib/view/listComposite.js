"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.ListCompositeTreeView = void 0;
const arrayComposite_1 = require("./arrayComposite");
class ListCompositeTreeView extends arrayComposite_1.ArrayCompositeTreeView {
    constructor(type, tree) {
        super(type, tree);
        this.type = type;
        this.tree = tree;
    }
    /**
     * Adds one view element at the end of the array and adds 1 to the current Tree length.
     */
    push(view) {
        const length = this.length;
        if (length >= this.type.limit) {
            throw Error("Error pushing over limit");
        }
        this.type.tree_setLength(this.tree, length + 1);
        // No need for pre-initialization like in ListBasic.push since ArrayCompositeTreeView.set() doesn't do a get node
        this.set(length, view);
    }
}
exports.ListCompositeTreeView = ListCompositeTreeView;
//# sourceMappingURL=listComposite.js.map