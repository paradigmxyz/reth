var UsesImports = artifacts.require("UsesImports");

contract("UsesImports", function(accounts) {
  let instance;

  before(async () => instance = await UsesImports.new());

  it('uses a method from a relative import', async () => {
    await instance.wrapsRelativePathMethod();
  })

  it('uses an import from node_modules', async () => {
    await instance.wrapsNodeModulesMethod();
  })

});
