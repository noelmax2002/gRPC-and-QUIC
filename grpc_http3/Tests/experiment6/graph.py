import os
import re
import matplotlib.pyplot as plt
import numpy as np

def extract_data(file_name):
    x_vals = []
    y_vals = []
    
    with open(file_name, 'r') as file:
        for line in file:
            match = re.match(r"Time elapsed: \((\d+\.\d+) ,(\d+\.\d+)\)", line)
            if match:
                x_vals.append(float(match.group(1)))
                y_vals.append(float(match.group(2)) * 1000)
    
    return x_vals, y_vals

def process_folder(folder_path):
    if folder_path == "":
        print("No folder path provided.")
        return [], [], []
    
    all_x_vals = []
    all_y_vals = []
    max_length = 0
    
    for file_name in os.listdir(folder_path):
        file_path = os.path.join(folder_path, file_name)
        if os.path.isfile(file_path):
            print(f"Processing file: {file_name}")
            x_vals, y_vals = extract_data(file_path)
            max_length = max(max_length, len(x_vals))
            all_x_vals.append(x_vals)
            all_y_vals.append(y_vals)
    
    x_mean = []
    y_mean = []
    y_std = []

    for i in range(max_length):
        x_vals_at_index = []
        y_vals_at_index = []
        
        for x_vals, y_vals in zip(all_x_vals, all_y_vals):
            if i < len(x_vals):
                x_vals_at_index.append(x_vals[i])
                y_vals_at_index.append(y_vals[i])
        
        if x_vals_at_index:
            x_mean.append(np.median(x_vals_at_index))
        if y_vals_at_index:
            y_mean.append(np.median(y_vals_at_index))
            y_std.append(np.std(y_vals_at_index))
    
    return x_mean, y_mean, y_std

def plot_comparison(x_mean1, y_mean1, y_std1, x_mean2, y_mean2, y_std2, x_mean3, y_mean3, y_std3 , folder1_name, folder2_name, title):
    plt.figure(figsize=(10, 6))

    if len(x_mean1) != 0:
        plt.plot(x_mean1, y_mean1, marker='o', linestyle='-', color='b', label="MPQUIC [A-A] - last scheduler")
        plt.fill_between(x_mean1, np.array(y_mean1), np.array(y_mean1) + np.array(y_std1), color='b', alpha=0.2)

    if len(x_mean2) != 0:
        plt.plot(x_mean2, y_mean2, marker='.', linestyle='--', color='g', label="MPQUIC [A-A] - lrtt scheduler")
        plt.fill_between(x_mean2, np.array(y_mean2), np.array(y_mean2) + np.array(y_std2), color='g', alpha=0.2)

    if len(x_mean3) != 0:
        plt.plot(x_mean3, y_mean3, marker='p', linestyle='-', color='y', label="MPQUIC [A-P] - simple scheduler")
        plt.fill_between(x_mean3, np.array(y_mean3), np.array(y_mean3) + np.array(y_std3), color='y', alpha=0.2)

    plt.axvline(x=8, color='black', linestyle='solid')
    plt.axvline(x=13, color='black', linestyle='solid')
    plt.ylim(0, 600)
    plt.yticks(np.arange(0, 601, 50))
    plt.xlabel('Time (s)')
    plt.ylabel('Delay of msg exchange (ms)')
    plt.grid(True)
    plt.legend(loc='upper right')
    plt.savefig('plot.pdf')

# Main execution
def main():
    folder_path1 = "quic_latency"
    folder_path2 = "quic_latency_lrtt"
    folder_path3 = "quic_latency_normal"
    folder_path1 = "quic_cut"
    folder_path2 = "quic_cut_lrtt"
    folder_path3 = "quic_cut_normal"
    folder_path1 = "LossResults/quic_cut_loss/last"
    folder_path2 = "LossResults/quic_cut_loss/lrtt"
    folder_path3 = "LossResults/quic_cut_loss/normal"
    folder_path1 = "Results/cut/last"
    folder_path2 = "Results/cut/lrtt"
    folder_path3 = "Results/cut/normal"
    folder_path1 = "Results/latency/last"
    folder_path2 = "Results/latency/lrtt"
    folder_path3 = "Results/latency/normal"
    #title = input("Enter the title for the plot: ")
    title=""

    x_mean1, y_mean1, y_std1 = process_folder(folder_path1)
    x_mean2, y_mean2, y_std2 = process_folder(folder_path2)
    x_mean3, y_mean3, y_std3 = process_folder(folder_path3)

    plot_comparison(x_mean1, y_mean1, y_std1,
                    x_mean2, y_mean2, y_std2,
                    x_mean3, y_mean3, y_std3,
                    os.path.basename(folder_path1), os.path.basename(folder_path2), title)

if __name__ == "__main__":
    main()
