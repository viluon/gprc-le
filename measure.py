import subprocess

# load required topologies line by line
with open("required.txt", "r") as f:
    for line in f:
        # replace commas with spaces
        line = line.replace(",", " ")
        # run the program target/debug/grpc-le
        process = subprocess.Popen(
            ["target/debug/grpc-le"],
            text = True,
            stdin  = subprocess.PIPE,
            stdout = subprocess.PIPE,
            stderr = subprocess.PIPE,
        )
        try:
            # pass the line on stdin
            process.communicate(line, timeout = 5)
        except subprocess.TimeoutExpired:
            process.terminate()
            stdout, _ = process.communicate()
            n_msgs = stdout.count("\n")
            print(f"{len(line.split(' '))},{n_msgs}")
