# Clone your fork locally
git clone https://github.com/yourusername/reth.git
cd reth

# Add the original repo as an upstream remote
git remote add upstream https://github.com/paradigmxyz/reth.git
git fetch upstream

# Make your changes in a branch
git checkout -b sven
# edit code, commit changes
git push origin sven

# Updating from upstream without losing changes
git fetch upstream
git checkout sven
git merge upstream/main   # or upstream/master depending on branch name

# resolve conflicts if any
git push origin sven

# Build a custom Docker image
docker build -t my-reth:latest .

docker tag my-reth:latest my-dockerhub-user/my-reth:latest
docker push my-dockerhub-user/my-reth:latest

# run your custom image
docker pull my-dockerhub-user/my-reth:latest
docker stop reth
docker rm reth
docker run ... my-dockerhub-user/my-reth:latest

# Custom method

rpc -> rpc-eth-api -> core.rs - Core RPC methods trait + implementations

rpc -> rpc-eth-api -> custom.rs - Custom RPC methods trait + implementations

rpc -> rpc-eth-api -> lib.rs - Import Mods